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
                    let Ok(cron) = cron::Schedule::from_str(&schedule.cron) else {
                        continue;
                    };
                    let due = cron.after(&previous).next().is_some_and(|next| next <= now);
                    if due {
                        let result = start_system_run(
                            &state,
                            &service.name,
                            &schedule.system,
                            request(schedule),
                            RunTrigger::Scheduled,
                            Some(schedule.name.clone()),
                        );
                        if let Err((_, message)) = result {
                            eprintln!(
                                "weave-server: schedule {}/{} skipped: {message}",
                                service.name, schedule.name
                            );
                        }
                    }
                }
            }
            previous = now;
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
