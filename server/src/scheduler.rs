//! Service schedule evaluation and the manual "run now" endpoint.

use std::str::FromStr as _;
use std::time::Duration;

use axum::Extension;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::Response;
use chrono::Utc;
use forge_server::{RequireClaims, err, ok};
use serde_json::json;

use crate::state::SharedState;
use crate::sysruns::{Action, RunTrigger, SysRunRequest, start_system_run};
use crate::systems::ScheduleDef;

/// Whether a six-field cron expression fires in `(previous, now]`.
/// Invalid expressions are never due.
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

fn request(schedule: &ScheduleDef) -> SysRunRequest {
    SysRunRequest {
        action: if schedule.action == "apply" {
            Action::Apply
        } else {
            Action::Check
        },
        playbook: schedule.playbook.clone(),
        play: schedule.play.clone(),
        keep: false,
    }
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
            let services = state.services.lock().unwrap().clone();
            for service in services {
                for schedule in service.schedules.iter().filter(|s| s.enabled) {
                    if cron_due(&schedule.cron, &previous, &now) {
                        let result = start_system_run(
                            &state,
                            &service.name,
                            &schedule.system,
                            request(schedule),
                            RunTrigger::Scheduled,
                            Some(schedule.name.clone()),
                        );
                        metrics::counter!(
                            crate::monitoring::SCHEDULE_DISPATCH_TOTAL,
                            "service" => service.name.clone(),
                            "schedule" => schedule.name.clone(),
                            "outcome" => if result.is_ok() { "started" } else { "skipped" },
                        )
                        .increment(1);
                        if let Err((_, message)) = result {
                            tracing::warn!(
                                service = %service.name,
                                schedule = %schedule.name,
                                %message,
                                "schedule skipped"
                            );
                        }
                    }
                }
            }
            // Remote repositories with a due sync_cron: spawned so a slow
            // network fetch never blocks the tick, sequential inside the
            // task (the git lock serializes cache mutations anyway).
            let due_repos: Vec<crate::repos::RepoDef> = {
                let repos = state.repos.lock().unwrap();
                repos
                    .iter()
                    .filter(|r| {
                        r.sync_cron
                            .as_deref()
                            .is_some_and(|c| cron_due(c, &previous, &now))
                    })
                    .cloned()
                    .collect()
            };
            if !due_repos.is_empty() {
                let state = state.clone();
                tokio::spawn(async move {
                    for repo in due_repos {
                        let dest = crate::repos::cache_dir(&state, &repo.name);
                        let result = {
                            let _guard = state.repo_git_lock.lock().await;
                            crate::repos::sync_repo(&repo, &dest).await
                        };
                        let outcome = match &result {
                            Ok(crate::repos::SyncOutcome::Synced) => "synced",
                            Ok(crate::repos::SyncOutcome::Skipped(_)) => "skipped",
                            Err(_) => "failed",
                        };
                        metrics::counter!(
                            crate::monitoring::REPO_SYNC_DISPATCH_TOTAL,
                            "repo" => repo.name.clone(),
                            "trigger" => "cron",
                            "outcome" => outcome,
                        )
                        .increment(1);
                        match result {
                            Ok(crate::repos::SyncOutcome::Synced) => {
                                tracing::info!(repo = %repo.name, "scheduled sync pulled the remote")
                            }
                            Ok(crate::repos::SyncOutcome::Skipped(msg)) => {
                                tracing::info!(repo = %repo.name, %msg, "scheduled sync skipped")
                            }
                            Err(e) => {
                                tracing::warn!(repo = %repo.name, "scheduled sync failed: {e}")
                            }
                        }
                    }
                });
            }
            previous = now;
            metrics::gauge!(crate::monitoring::SCHEDULER_LAST_TICK).set(now.timestamp() as f64);
        }
    });
}

pub async fn run_now(
    Extension(state): Extension<SharedState>,
    UrlPath((service, name)): UrlPath<(String, String)>,
    _claims: RequireClaims,
) -> Response {
    let schedule = {
        let services = state.services.lock().unwrap();
        let Some(service_def) = services.iter().find(|s| s.name == service) else {
            return err(StatusCode::NOT_FOUND, "no such service");
        };
        let Some(schedule) = service_def.schedules.iter().find(|s| s.name == name) else {
            return err(StatusCode::NOT_FOUND, "no such schedule");
        };
        schedule.clone()
    };
    match start_system_run(
        &state,
        &service,
        &schedule.system,
        request(&schedule),
        RunTrigger::Manual,
        Some(schedule.name),
    ) {
        Ok(run) => ok(json!({ "id": run.id })),
        Err((status, message)) => err(status, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn cron_due_fires_once_per_boundary() {
        // Every 15 minutes; the tick window straddles :15.
        let expr = "0 */15 * * * *";
        let before = Utc.with_ymd_and_hms(2026, 1, 1, 10, 14, 50).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 5).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 20).unwrap();
        assert!(cron_due(expr, &before, &at));
        // The next window starts where the last ended — no double fire.
        assert!(!cron_due(expr, &at, &after));
    }

    #[test]
    fn invalid_expressions_are_never_due() {
        let previous = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        assert!(!cron_due("not a cron", &previous, &now));
    }
}
