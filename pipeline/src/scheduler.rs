//! Scheduled triggers: a 15s tick evaluates every enabled `type="schedule"`
//! trigger's six-field cron and starts a run when it is due. Mirrors
//! weave-server's scheduler.

use std::collections::HashMap;
use std::str::FromStr as _;
use std::time::Duration;

use chrono::Utc;

use crate::state::SharedState;

/// Whether a six-field cron expression fires in `(previous, now]`. Invalid
/// expressions are never due.
pub(crate) fn cron_due(
    expr: &str,
    previous: &chrono::DateTime<Utc>,
    now: &chrono::DateTime<Utc>,
) -> bool {
    let Ok(cron) = cron::Schedule::from_str(expr) else {
        return false;
    };
    cron.after(previous).next().is_some_and(|next| next <= *now)
}

pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let mut previous = Utc::now();
        loop {
            interval.tick().await;
            let now = Utc::now();
            // Collect due (pipeline, trigger, bindings) tuples under the lock,
            // then start runs without holding it.
            let due: Vec<(String, String, HashMap<String, String>)> = {
                let pipelines = state.pipelines.lock().unwrap();
                let mut out = Vec::new();
                for p in pipelines.iter() {
                    for t in p
                        .triggers
                        .iter()
                        .filter(|t| t.enabled && t.r#type == "schedule")
                    {
                        if t
                            .cron
                            .as_deref()
                            .is_some_and(|c| cron_due(c, &previous, &now))
                        {
                            out.push((
                                p.name.clone(),
                                t.name.clone(),
                                t.bindings.iter().cloned().collect(),
                            ));
                        }
                    }
                }
                out
            };
            for (pipeline, trigger, bindings) in due {
                match crate::trigger::start_run(
                    &state,
                    &pipeline,
                    &bindings,
                    format!("schedule:{trigger}"),
                ) {
                    Ok(run) => {
                        tracing::info!(pipeline = %pipeline, trigger = %trigger, run_id = %run.id, "scheduled run started")
                    }
                    Err((_, message)) => {
                        tracing::warn!(pipeline = %pipeline, trigger = %trigger, %message, "scheduled run skipped")
                    }
                }
            }
            previous = now;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn cron_due_fires_once_per_boundary() {
        let expr = "0 */15 * * * *";
        let before = Utc.with_ymd_and_hms(2026, 1, 1, 10, 14, 50).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 5).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 20).unwrap();
        assert!(cron_due(expr, &before, &at));
        assert!(!cron_due(expr, &at, &after));
    }

    #[test]
    fn invalid_expressions_never_due() {
        let previous = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        assert!(!cron_due("not a cron", &previous, &now));
    }
}
