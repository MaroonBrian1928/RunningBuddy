use crate::AppState;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde_json::json;

/// Convert free-text training plan into a structured week/day schedule via the
/// configured LLM provider. Best-effort: returns None when no provider is
/// configured or the call fails, and callers must treat that as "no schedule".
pub async fn structure_training_plan(
    state: &AppState,
    plan_text: &str,
) -> Option<serde_json::Value> {
    let result = match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_structure_plan(state, plan_text).await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_structure_plan(state, plan_text).await
        }
        _ => return None,
    };
    match result {
        Ok(schedule) if schedule_is_usable(&schedule) => Some(schedule),
        Ok(_) => {
            tracing::warn!("structured plan response was empty; storing no schedule");
            None
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to structure training plan; storing no schedule");
            None
        }
    }
}

fn schedule_is_usable(schedule: &serde_json::Value) -> bool {
    schedule
        .get("weeks")
        .and_then(|weeks| weeks.as_array())
        .map(|weeks| !weeks.is_empty())
        .unwrap_or(false)
}

const STRUCTURE_PLAN_PROMPT: &str = r#"Convert the runner's free-text training plan into a structured schedule. Rules:
- Emit one entry per week, and one entry per scheduled day within the week. Omit days with nothing scheduled (do not invent rest days unless the plan names them).
- week numbers start at 1 = the first week of the plan.
- day_of_week must be one of Monday..Sunday. If the plan does not assign workouts to specific weekdays, distribute them in the plan's stated order across the week starting Monday and set day_is_approximate to true for those entries.
- description is the workout as written, lightly normalized.
- workout_type is one of: easy, long, tempo, intervals, race, rest, cross_training, strength, other.
- run_miles is the planned run distance in miles, or null when not a run or no distance is given.
- Do not add coaching commentary. Structure only what the plan says."#;

fn structure_plan_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["weeks"],
        "properties": {
            "weeks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["week", "days"],
                    "properties": {
                        "week": { "type": "integer" },
                        "days": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["day_of_week", "description", "workout_type", "run_miles", "day_is_approximate"],
                                "properties": {
                                    "day_of_week": { "type": "string", "enum": ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"] },
                                    "description": { "type": "string" },
                                    "workout_type": { "type": "string", "enum": ["easy", "long", "tempo", "intervals", "race", "rest", "cross_training", "strength", "other"] },
                                    "run_miles": { "type": ["number", "null"] },
                                    "day_is_approximate": { "type": "boolean" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn structure_plan_schema_gemini() -> serde_json::Value {
    json!({
        "type": "OBJECT",
        "required": ["weeks"],
        "properties": {
            "weeks": {
                "type": "ARRAY",
                "items": {
                    "type": "OBJECT",
                    "required": ["week", "days"],
                    "properties": {
                        "week": { "type": "INTEGER" },
                        "days": {
                            "type": "ARRAY",
                            "items": {
                                "type": "OBJECT",
                                "required": ["day_of_week", "description", "workout_type", "day_is_approximate"],
                                "properties": {
                                    "day_of_week": { "type": "STRING" },
                                    "description": { "type": "STRING" },
                                    "workout_type": { "type": "STRING" },
                                    "run_miles": { "type": "NUMBER", "nullable": true },
                                    "day_is_approximate": { "type": "BOOLEAN" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

async fn openai_structure_plan(
    state: &AppState,
    plan_text: &str,
) -> anyhow::Result<serde_json::Value> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let payload = json!({
        "model": state.config.llm_model,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "plan_schedule",
                "strict": true,
                "schema": structure_plan_schema(),
            }
        },
        "messages": [
            { "role": "system", "content": STRUCTURE_PLAN_PROMPT },
            { "role": "user", "content": plan_text }
        ]
    });
    let response: serde_json::Value = state
        .http
        .post(format!(
            "{}/chat/completions",
            state.config.openai_base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("OpenAI plan structuring returned no content"))?;
    Ok(serde_json::from_str(content)?)
}

async fn gemini_structure_plan(
    state: &AppState,
    plan_text: &str,
) -> anyhow::Result<serde_json::Value> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!("{}\n\n{}", STRUCTURE_PLAN_PROMPT, plan_text) }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": structure_plan_schema_gemini(),
        }
    });
    let response: serde_json::Value = state
        .http
        .post(format!(
            "{}/models/{}:generateContent?key={}",
            state.config.gemini_base_url.trim_end_matches('/'),
            state.config.llm_model,
            api_key
        ))
        .json(&payload)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let content = response["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Gemini plan structuring returned no content"))?;
    Ok(serde_json::from_str(content)?)
}

/// Compare the structured schedule against synced activities and produce a
/// day-by-day evaluation for a window of recent past days plus the next week.
/// `activities` are the compact summaries built for the advice payload (they
/// carry `local_date`). Statuses are computed here so the LLM never has to
/// decide from timestamps whether a workout was skipped.
pub fn evaluate_schedule(
    schedule: &serde_json::Value,
    plan_start_date: &str,
    activities: &[serde_json::Value],
    today: NaiveDate,
) -> Option<serde_json::Value> {
    let start = NaiveDate::parse_from_str(plan_start_date.get(..10)?, "%Y-%m-%d").ok()?;
    let weeks = schedule.get("weeks")?.as_array()?;

    let window_start = today - Duration::days(14);
    let window_end = today + Duration::days(7);

    let mut entries = Vec::new();
    for week in weeks {
        let week_number = week.get("week").and_then(|value| value.as_i64())?;
        for day in week.get("days").and_then(|days| days.as_array())? {
            let Some(weekday) = day
                .get("day_of_week")
                .and_then(|value| value.as_str())
                .and_then(parse_weekday)
            else {
                continue;
            };
            let date = schedule_date(start, week_number, weekday);
            if date < window_start || date > window_end {
                continue;
            }
            entries.push((date, week_number, day.clone(), workout_kind(day)));
        }
    }
    if entries.is_empty() {
        return None;
    }
    entries.sort_by_key(|(date, ..)| *date);

    let mut candidates = activity_candidates(activities);
    let assignments = assign_activities(&entries, &mut candidates);

    let days = entries
        .iter()
        .zip(&assignments)
        .map(|((date, week_number, day, kind), assigned)| {
            let matched = assigned.map(|index| &candidates[index]);
            let status = match (*kind, matched.as_ref()) {
                ("rest", _) => "rest",
                (_, Some(candidate)) => match (candidate.date - *date).num_days() {
                    0 => "completed",
                    offset if offset < 0 => "completed_early",
                    _ => "completed_late",
                },
                (_, None) if *date > today => "upcoming",
                (_, None) if *date == today => "pending_today",
                // Still inside the ±1-day match window: could be done today.
                (_, None) if *date == today - Duration::days(MATCH_DAY_TOLERANCE) => {
                    "pending_makeup"
                }
                (_, None) => "missed",
            };
            json!({
                "date": date.to_string(),
                "day_of_week": date.format("%A").to_string(),
                "plan_week": week_number,
                "planned": day,
                "status": status,
                "completed_on": matched.map(|candidate| candidate.date.to_string()),
                "matched_activity": matched.map(|candidate| candidate.summary.clone()),
            })
        })
        .collect::<Vec<_>>();

    Some(json!({
        "notes": "Authoritative day-by-day comparison of the structured plan against synced activities, computed by the app. Trust these statuses; do not re-derive them from timestamps. Workouts are matched up to one day early or late: completed_early/completed_late mean the workout was done, just shifted (see completed_on). pending_today = scheduled today and not done yet — never call it skipped. pending_makeup = scheduled yesterday, unmatched, but still completable a day late. Each synced activity satisfies at most one planned workout. Dates outside the last 14 / next 7 days are omitted.",
        "days": days,
    }))
}

const MATCH_DAY_TOLERANCE: i64 = 1;

struct Candidate {
    date: NaiveDate,
    is_run: bool,
    distance_miles: Option<f64>,
    claimed: bool,
    summary: serde_json::Value,
}

/// Run-type workouts must be satisfied by runs; rest days match nothing;
/// everything else (cross_training, strength, other) is flexible.
fn workout_kind(day: &serde_json::Value) -> &'static str {
    match day.get("workout_type").and_then(|value| value.as_str()) {
        Some("rest") => "rest",
        Some("easy" | "long" | "tempo" | "intervals" | "race") => "run",
        _ if day.get("run_miles").and_then(|value| value.as_f64()).is_some() => "run",
        _ => "flex",
    }
}

fn activity_candidates(activities: &[serde_json::Value]) -> Vec<Candidate> {
    activities
        .iter()
        .filter_map(|activity| {
            let date = activity.get("local_date")?.as_str()?;
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            let sport_type = activity.get("sport_type").and_then(|value| value.as_str());
            Some(Candidate {
                date,
                is_run: sport_type
                    .map(|sport| sport.to_ascii_lowercase().contains("run"))
                    .unwrap_or(false),
                distance_miles: activity.get("distance_miles").and_then(|value| value.as_f64()),
                claimed: false,
                summary: json!({
                    "name": activity.get("name"),
                    "sport_type": sport_type,
                    "local_date": date.to_string(),
                    "distance_miles": activity.get("distance_miles"),
                    "moving_time_seconds": activity.get("moving_time_seconds"),
                }),
            })
        })
        .collect()
}

/// Greedy matching in four passes so exact-day matches win before shifted
/// ones, and run workouts get first claim on run activities before flexible
/// workouts (cross-training etc.) can take anything.
fn assign_activities(
    entries: &[(NaiveDate, i64, serde_json::Value, &'static str)],
    candidates: &mut [Candidate],
) -> Vec<Option<usize>> {
    let mut assignments: Vec<Option<usize>> = vec![None; entries.len()];
    for (kind_filter, tolerance) in [
        ("run", 0),
        ("run", MATCH_DAY_TOLERANCE),
        ("flex", 0),
        ("flex", MATCH_DAY_TOLERANCE),
    ] {
        for (entry_index, (date, _, day, kind)) in entries.iter().enumerate() {
            if *kind != kind_filter || assignments[entry_index].is_some() {
                continue;
            }
            let target_miles = day.get("run_miles").and_then(|value| value.as_f64());
            let best = candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| !candidate.claimed)
                .filter(|(_, candidate)| {
                    (candidate.date - *date).num_days().abs() <= tolerance
                        && (kind_filter != "run" || candidate.is_run)
                })
                .min_by(|(_, a), (_, b)| {
                    candidate_rank(a, *date, target_miles, kind_filter)
                        .partial_cmp(&candidate_rank(b, *date, target_miles, kind_filter))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(index, _)| index);
            if let Some(candidate_index) = best {
                candidates[candidate_index].claimed = true;
                assignments[entry_index] = Some(candidate_index);
            }
        }
    }
    assignments
}

/// Lower ranks first: closest day, then (for flexible workouts) prefer
/// non-runs, then closest distance to the planned mileage.
fn candidate_rank(
    candidate: &Candidate,
    date: NaiveDate,
    target_miles: Option<f64>,
    kind_filter: &str,
) -> (i64, i64, f64) {
    let distance_gap = match (target_miles, candidate.distance_miles) {
        (Some(target), Some(actual)) => (actual - target).abs(),
        _ => 0.0,
    };
    (
        (candidate.date - date).num_days().abs(),
        if kind_filter == "flex" && candidate.is_run { 1 } else { 0 },
        distance_gap,
    )
}

/// Week N, day D maps to the date with weekday D inside the 7-day block that
/// starts at plan_start + (N-1) weeks.
fn schedule_date(start: NaiveDate, week_number: i64, weekday: Weekday) -> NaiveDate {
    let block_start = start + Duration::weeks(week_number - 1);
    let offset = (7 + weekday.num_days_from_monday() as i64
        - block_start.weekday().num_days_from_monday() as i64)
        % 7;
    block_start + Duration::days(offset)
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    value.parse::<Weekday>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schedule() -> serde_json::Value {
        json!({
            "weeks": [
                { "week": 3, "days": [
                    { "day_of_week": "Friday", "description": "Long run 4 miles", "workout_type": "long", "run_miles": 4.0, "day_is_approximate": false },
                    { "day_of_week": "Sunday", "description": "Easy 3 miles", "workout_type": "easy", "run_miles": 3.0, "day_is_approximate": false }
                ]}
            ]
        })
    }

    #[test]
    fn schedule_date_maps_week_and_day() {
        // Plan starts Monday 2026-06-15; week 3 Friday = 2026-07-03.
        let start = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        assert_eq!(
            schedule_date(start, 3, Weekday::Fri).to_string(),
            "2026-07-03"
        );
        // Plan starting mid-week: week 1 block is Wed..Tue, so week 1 Monday
        // falls at the end of that block.
        let wednesday_start = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();
        assert_eq!(
            schedule_date(wednesday_start, 1, Weekday::Mon).to_string(),
            "2026-06-22"
        );
    }

    #[test]
    fn evaluate_schedule_marks_today_pending_not_missed() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 3).unwrap();
        let evaluation =
            evaluate_schedule(&sample_schedule(), "2026-06-15", &[], today).unwrap();
        let days = evaluation["days"].as_array().unwrap();
        assert_eq!(days[0]["date"], "2026-07-03");
        assert_eq!(days[0]["status"], "pending_today");
        assert_eq!(days[1]["status"], "upcoming");
    }

    #[test]
    fn evaluate_schedule_matches_activity_on_local_date() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 4).unwrap();
        let activities = vec![json!({
            "name": "Friday long run",
            "sport_type": "Run",
            "local_date": "2026-07-03",
            "distance_miles": 4.6,
            "moving_time_seconds": 2700
        })];
        let evaluation =
            evaluate_schedule(&sample_schedule(), "2026-06-15", &activities, today).unwrap();
        let days = evaluation["days"].as_array().unwrap();
        assert_eq!(days[0]["status"], "completed");
        assert_eq!(days[0]["matched_activity"]["distance_miles"], 4.6);
        // Sunday easy run is still in the future.
        assert_eq!(days[1]["status"], "upcoming");
    }

    #[test]
    fn long_run_done_a_day_early_matches_and_is_not_missed() {
        // Saturday cross-train + Sunday long run planned; the runner does the
        // long run on Saturday instead. The run workout must claim the run
        // (not the flexible cross-train day), shifted by one day.
        let schedule = json!({
            "weeks": [
                { "week": 1, "days": [
                    { "day_of_week": "Saturday", "description": "30m cross-train", "workout_type": "cross_training", "run_miles": null, "day_is_approximate": false },
                    { "day_of_week": "Sunday", "description": "Long run 4 miles", "workout_type": "long", "run_miles": 4.0, "day_is_approximate": false }
                ]}
            ]
        });
        let activities = vec![json!({
            "name": "Saturday run",
            "sport_type": "Run",
            "local_date": "2026-07-04",
            "distance_miles": 4.6,
            "moving_time_seconds": 2700
        })];
        let today = NaiveDate::from_ymd_opt(2026, 7, 4).unwrap();
        let evaluation =
            evaluate_schedule(&schedule, "2026-06-29", &activities, today).unwrap();
        let days = evaluation["days"].as_array().unwrap();
        // Saturday cross-train: its only candidate was claimed by the long
        // run, and the day is not over.
        assert_eq!(days[0]["status"], "pending_today");
        // Sunday long run: completed a day early.
        assert_eq!(days[1]["status"], "completed_early");
        assert_eq!(days[1]["completed_on"], "2026-07-04");
        assert_eq!(days[1]["matched_activity"]["distance_miles"], 4.6);
    }

    #[test]
    fn run_done_a_day_late_matches_as_completed_late() {
        let schedule = json!({
            "weeks": [
                { "week": 1, "days": [
                    { "day_of_week": "Tuesday", "description": "Easy 3 miles", "workout_type": "easy", "run_miles": 3.0, "day_is_approximate": false }
                ]}
            ]
        });
        let activities = vec![json!({
            "name": "Wednesday run",
            "sport_type": "Run",
            "local_date": "2026-07-01",
            "distance_miles": 3.1,
            "moving_time_seconds": 1700
        })];
        let today = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap();
        let evaluation =
            evaluate_schedule(&schedule, "2026-06-29", &activities, today).unwrap();
        let days = evaluation["days"].as_array().unwrap();
        assert_eq!(days[0]["date"], "2026-06-30");
        assert_eq!(days[0]["status"], "completed_late");
        assert_eq!(days[0]["completed_on"], "2026-07-01");
    }

    #[test]
    fn unmatched_yesterday_is_pending_makeup_not_missed() {
        let schedule = json!({
            "weeks": [
                { "week": 1, "days": [
                    { "day_of_week": "Tuesday", "description": "Easy 3 miles", "workout_type": "easy", "run_miles": 3.0, "day_is_approximate": false }
                ]}
            ]
        });
        let today = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(); // Wednesday
        let evaluation = evaluate_schedule(&schedule, "2026-06-29", &[], today).unwrap();
        let days = evaluation["days"].as_array().unwrap();
        assert_eq!(days[0]["status"], "pending_makeup");

        // Two days later it becomes a real miss.
        let later = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap();
        let evaluation = evaluate_schedule(&schedule, "2026-06-29", &[], later).unwrap();
        assert_eq!(evaluation["days"][0]["status"], "missed");
    }
}
