//! 工数の記録。1 日の勤務時間（残業も含めた実際の時間）を、タスクごとの割合で振り分ける。
//!
//! 振り分けは 30 分刻みで、合計が勤務時間にぴったり合うように端数を配る（最大剰余方式）。
//! 時間は分で持ち、人日への換算（1D = 何時間）は設定 `minutes_per_day` で行う。

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{ServiceError, ServiceResult, SettingKey, Tx, now_text};
use crate::model::TaskStatus;

/// 振り分けの刻み（分）
pub const STEP_MIN: i64 = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 1D（1 人日）の時間（分）
    pub minutes_per_day: i64,
    /// 新しい日の勤務時間の初期値（分）
    pub default_work_min: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct SettingsPatch {
    pub minutes_per_day: Option<i64>,
    pub default_work_min: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayEntry {
    pub task_id: i64,
    pub weight: f64,
    pub minutes: i64,
}

str_enum! {
    /// time_entry の記録元
    pub enum EntrySource {
        /// 1 日の勤務時間を割合で振り分けたもの
        Daily => "daily",
        /// タイマー（将来用）
        Timer => "timer",
        /// 手入力（将来用）
        Manual => "manual",
    }
}

str_enum! {
    /// 工数記録の候補に出す理由
    pub enum SuggestReason {
        /// その日に記録済み
        Recorded => "recorded",
        /// その日に完了した
        Completed => "completed",
        /// 線表でその日にバーが重なる
        Planned => "planned",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub task_id: i64,
    pub reason: SuggestReason,
    /// 記録がなくても候補に出る（その日に完了・線表でその日にバーが重なる）。
    /// 画面で振り分けから外しても戻ってくるので、外すボタンを出さない
    pub always_suggested: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkDay {
    pub date: String,
    /// その日の勤務時間。まだ記録していなければ null
    pub worked_min: Option<i64>,
    pub entries: Vec<DayEntry>,
    /// 振り分け済みの合計（タスクを消すと勤務時間より少なくなることがある）
    pub allocated_min: i64,
    /// その日に関わったタスク（振り分けの候補）
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaySummary {
    pub date: String,
    pub worked_min: i64,
    pub allocated_min: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Allocation {
    pub task_id: i64,
    pub weight: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveDay {
    pub worked_min: i64,
    #[serde(default)]
    pub allocations: Vec<Allocation>,
}

fn check_date(date: &str) -> ServiceResult<()> {
    super::parse_date(date).map(|_| ())
}

// ---------- 設定 ----------
fn setting_i64(conn: &Connection, key: SettingKey, default: i64) -> ServiceResult<i64> {
    Ok(super::get_setting(conn, key)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(default))
}

pub fn get_settings(conn: &Connection) -> ServiceResult<Settings> {
    Ok(Settings {
        minutes_per_day: setting_i64(conn, SettingKey::MinutesPerDay, 480)?,
        default_work_min: setting_i64(conn, SettingKey::DefaultWorkMin, 480)?,
    })
}

pub fn update_settings(conn: &Connection, patch: SettingsPatch) -> ServiceResult<Settings> {
    let tx = Tx::begin(conn)?;
    if let Some(m) = patch.minutes_per_day {
        if !(60..=24 * 60).contains(&m) {
            return Err(ServiceError::Invalid(
                "1D は 1〜24 時間にしてください".into(),
            ));
        }
        super::set_setting(&tx, SettingKey::MinutesPerDay, &m.to_string())?;
    }
    if let Some(m) = patch.default_work_min {
        if !(0..=24 * 60).contains(&m) || m % STEP_MIN != 0 {
            return Err(ServiceError::Invalid(
                "標準の勤務時間は 0〜24 時間の 0.5 時間刻みにしてください".into(),
            ));
        }
        super::set_setting(&tx, SettingKey::DefaultWorkMin, &m.to_string())?;
    }
    tx.commit()?;
    get_settings(conn)
}

// ---------- 振り分け ----------
/// 勤務時間を割合で 30 分刻みに振り分ける。合計は勤務時間に一致する。
/// 端数は小数部分の大きい順（同じなら割合の大きい順、先に並んだ順）に 30 分ずつ配る
/// 1 件の割合の上限（足しても桁あふれしない範囲）
const MAX_WEIGHT: f64 = 1_000_000.0;

pub fn allocate(worked_min: i64, weights: &[f64]) -> Vec<i64> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || worked_min <= 0 {
        return vec![0; weights.len()];
    }
    let units = worked_min / STEP_MIN;
    let raw: Vec<f64> = weights.iter().map(|w| w / total * units as f64).collect();
    let mut out: Vec<i64> = raw.iter().map(|r| r.floor() as i64).collect();
    let mut rest = units - out.iter().sum::<i64>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        let fa = raw[a] - raw[a].floor();
        let fb = raw[b] - raw[b].floor();
        fb.total_cmp(&fa)
            .then(weights[b].total_cmp(&weights[a]))
            .then(a.cmp(&b))
    });
    for i in order {
        if rest == 0 {
            break;
        }
        if weights[i] > 0.0 {
            out[i] += 1;
            rest -= 1;
        }
    }
    out.into_iter().map(|u| u * STEP_MIN).collect()
}

// ---------- 1 日 ----------
pub fn get_day(conn: &Connection, date: &str) -> ServiceResult<WorkDay> {
    check_date(date)?;
    let worked_min: Option<i64> = conn
        .query_row(
            "SELECT worked_min FROM work_day WHERE date = ?1",
            [date],
            |r| r.get(0),
        )
        .optional()?;
    let entries: Vec<DayEntry> = {
        let mut stmt = conn.prepare(
            "SELECT task_id, COALESCE(weight, 0), minutes FROM time_entry \
             WHERE work_date = ?1 AND source = ?2 ORDER BY id",
        )?;
        stmt.query_map(params![date, EntrySource::Daily], |r| {
            Ok(DayEntry {
                task_id: r.get(0)?,
                weight: r.get(1)?,
                minutes: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };
    let allocated_min = entries.iter().map(|e| e.minutes).sum();

    // 振り分けの候補：記録済み・その日に完了・線表でその日にバーが重なるもの（理由の優先順に 1 件ずつ）。
    // バーは線表と同じ考え方：開始は開始予定日（なければ着手日）、終了は終了予定日（完了なら完了日）。
    // 終了日だけなら ◆ の日、開始だけで未完了なら開始日から今日まで
    let today = super::today().to_string();
    let mut stmt = conn.prepare(
        "WITH span AS ( \
           SELECT id, status, kind, \
             COALESCE(planned_start, substr(started_at, 1, 10)) AS s, \
             COALESCE(planned_end, CASE WHEN status = ?6 THEN substr(completed_at, 1, 10) END) AS e \
           FROM task \
         ) \
         SELECT task_id, CASE MIN(p) WHEN 1 THEN ?3 WHEN 2 THEN ?4 ELSE ?5 END, MAX(p) > 1 FROM ( \
           SELECT task_id, ?3 AS reason, 1 AS p FROM time_entry WHERE work_date = ?1 \
           UNION ALL SELECT id, ?4, 2 FROM task WHERE substr(completed_at, 1, 10) = ?1 AND kind = ?7 \
           UNION ALL SELECT id, ?5, 3 FROM span WHERE kind = ?7 AND ( \
             (s IS NOT NULL AND e IS NOT NULL AND s <= ?1 AND e >= ?1) \
             OR (s IS NULL AND e = ?1) \
             OR (e IS NULL AND s IS NOT NULL AND status != ?6 AND s <= ?1 AND ?1 <= ?2)) \
         ) GROUP BY task_id ORDER BY MIN(p), task_id",
    )?;
    let suggestions = stmt
        .query_map(
            params![
                date,
                today,
                SuggestReason::Recorded,
                SuggestReason::Completed,
                SuggestReason::Planned,
                TaskStatus::Done,
                crate::model::TaskKind::Own
            ],
            |r| {
                Ok(Suggestion {
                    task_id: r.get(0)?,
                    reason: r.get(1)?,
                    always_suggested: r.get(2)?,
                })
            },
        )?
        .collect::<Result<_, _>>()?;

    Ok(WorkDay {
        date: date.to_string(),
        worked_min,
        entries,
        allocated_min,
        suggestions,
    })
}

/// その日の勤務時間と割合を保存し、各タスクの時間を計算し直す
pub fn save_day(conn: &Connection, date: &str, input: SaveDay) -> ServiceResult<WorkDay> {
    check_date(date)?;
    if !(0..=24 * 60).contains(&input.worked_min) || input.worked_min % STEP_MIN != 0 {
        return Err(ServiceError::Invalid(
            "勤務時間は 0〜24 時間の 0.5 時間刻みにしてください".into(),
        ));
    }
    let mut allocs: Vec<Allocation> = Vec::new();
    for a in input.allocations {
        // 大きすぎる値は合計が無限大になって振り分けが崩れるので弾く
        if !a.weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(&a.weight) {
            return Err(ServiceError::Invalid(format!(
                "割合は 0〜{MAX_WEIGHT} にしてください"
            )));
        }
        if a.weight == 0.0 {
            continue;
        }
        // 同じタスクが 2 回来たら割合を足す
        match allocs.iter_mut().find(|x| x.task_id == a.task_id) {
            Some(x) => x.weight += a.weight,
            None => allocs.push(a),
        }
    }
    for a in &allocs {
        super::task_service::get_task(conn, a.task_id)?;
    }

    let weights: Vec<f64> = allocs.iter().map(|a| a.weight).collect();
    let minutes = allocate(input.worked_min, &weights);
    let now = now_text();
    let tx = Tx::begin(conn)?;
    tx.execute(
        "DELETE FROM time_entry WHERE work_date = ?1 AND source = ?2",
        params![date, EntrySource::Daily],
    )?;
    if input.worked_min == 0 && allocs.is_empty() {
        tx.execute("DELETE FROM work_day WHERE date = ?1", [date])?;
    } else {
        tx.execute(
            "INSERT INTO work_day (date, worked_min) VALUES (?1, ?2) \
             ON CONFLICT(date) DO UPDATE SET worked_min = excluded.worked_min",
            params![date, input.worked_min],
        )?;
    }
    for (a, m) in allocs.iter().zip(&minutes) {
        tx.execute(
            "INSERT INTO time_entry (task_id, work_date, minutes, weight, source, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![a.task_id, date, m, a.weight, EntrySource::Daily, now],
        )?;
    }
    tx.commit()?;
    get_day(conn, date)
}

/// 期間内に記録した日の一覧
pub fn list_days(conn: &Connection, from: &str, to: &str) -> ServiceResult<Vec<DaySummary>> {
    check_date(from)?;
    check_date(to)?;
    let mut stmt = conn.prepare(
        "SELECT d.date, d.worked_min, \
           COALESCE((SELECT SUM(minutes) FROM time_entry e WHERE e.work_date = d.date), 0) \
         FROM work_day d WHERE d.date BETWEEN ?1 AND ?2 ORDER BY d.date",
    )?;
    let days = stmt
        .query_map([from, to], |r| {
            Ok(DaySummary {
                date: r.get(0)?,
                worked_min: r.get(1)?,
                allocated_min: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{NewTask, TaskPatch};
    use crate::service::{task_service as ts, undo_service};

    fn add(conn: &Connection, title: &str) -> i64 {
        ts::create_task(
            conn,
            NewTask {
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn allocate_sums_to_worked_time_in_half_hours() {
        assert_eq!(allocate(480, &[1.0, 1.0]), [240, 240]);
        // 10 時間を 1:1:1 → 3.5h・3.5h・3h（端数は先の方へ）
        assert_eq!(allocate(600, &[1.0, 1.0, 1.0]), [210, 210, 180]);
        assert_eq!(allocate(450, &[2.0, 1.0]), [300, 150]);
        // 割合の小さいものは 0 になることもあるが合計は合う
        let m = allocate(60, &[10.0, 1.0, 1.0]);
        assert_eq!(m.iter().sum::<i64>(), 60);
        assert!(m.iter().all(|x| x % 30 == 0));
        assert_eq!(allocate(480, &[]), Vec::<i64>::new());
        assert_eq!(allocate(480, &[0.0]), [0]);
    }

    #[test]
    fn save_day_records_minutes_and_task_actuals() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a");
        let b = add(&conn, "b");
        let day = save_day(
            &conn,
            "2026-11-20",
            SaveDay {
                worked_min: 600,
                allocations: vec![
                    Allocation {
                        task_id: a,
                        weight: 3.0,
                    },
                    Allocation {
                        task_id: b,
                        weight: 1.0,
                    },
                    Allocation {
                        task_id: b,
                        weight: 0.0,
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(day.worked_min, Some(600));
        assert_eq!(day.allocated_min, 600);
        let m: Vec<_> = day.entries.iter().map(|e| (e.task_id, e.minutes)).collect();
        assert_eq!(m, [(a, 450), (b, 150)]);
        assert_eq!(ts::get_task(&conn, a).unwrap().actual_min, 450);

        // 保存し直すと置き換わる
        save_day(
            &conn,
            "2026-11-20",
            SaveDay {
                worked_min: 480,
                allocations: vec![Allocation {
                    task_id: b,
                    weight: 1.0,
                }],
            },
        )
        .unwrap();
        assert_eq!(ts::get_task(&conn, a).unwrap().actual_min, 0);
        assert_eq!(ts::get_task(&conn, b).unwrap().actual_min, 480);
        let days = list_days(&conn, "2026-11-16", "2026-11-22").unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!((days[0].worked_min, days[0].allocated_min), (480, 480));
    }

    #[test]
    fn rejects_bad_input() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a");
        let save = |worked, w| {
            save_day(
                &conn,
                "2026-11-20",
                SaveDay {
                    worked_min: worked,
                    allocations: vec![Allocation {
                        task_id: a,
                        weight: w,
                    }],
                },
            )
        };
        assert!(matches!(save(470, 1.0), Err(ServiceError::Invalid(_))));
        assert!(matches!(save(480, -1.0), Err(ServiceError::Invalid(_))));
        assert!(matches!(
            save_day(
                &conn,
                "2026-11-20",
                SaveDay {
                    worked_min: 480,
                    allocations: vec![Allocation {
                        task_id: 999,
                        weight: 1.0
                    }],
                }
            ),
            Err(ServiceError::NotFound(999))
        ));
    }

    /// 他の人の作業は、日程が重なっていても候補に出さない
    #[test]
    fn other_peoples_tasks_are_not_suggested() {
        let conn = db::open_in_memory().unwrap();
        let mine = add(&conn, "自分");
        let theirs = add(&conn, "他の人");
        for id in [mine, theirs] {
            let p: TaskPatch = serde_json::from_str(
                r#"{"planned_start":"2030-01-08","planned_end":"2030-01-12"}"#,
            )
            .unwrap();
            ts::update_task(&conn, id, p).unwrap();
        }
        let p: TaskPatch = serde_json::from_str(r#"{"kind":"external"}"#).unwrap();
        ts::update_task(&conn, theirs, p).unwrap();
        let day = get_day(&conn, "2030-01-10").unwrap();
        let ids: Vec<i64> = day.suggestions.iter().map(|s| s.task_id).collect();
        assert_eq!(ids, [mine]);
    }

    /// 記録済みでも、線表で重なるものは「記録を消しても候補に残る」
    #[test]
    fn always_suggested_marks_tasks_that_come_back() {
        let conn = db::open_in_memory().unwrap();
        let planned = add(&conn, "予定期間");
        let other = add(&conn, "予定なし");
        let p: TaskPatch =
            serde_json::from_str(r#"{"planned_start":"2030-01-08","planned_end":"2030-01-12"}"#)
                .unwrap();
        ts::update_task(&conn, planned, p).unwrap();
        save_day(
            &conn,
            "2030-01-10",
            SaveDay {
                worked_min: 480,
                allocations: vec![
                    Allocation {
                        task_id: planned,
                        weight: 1.0,
                    },
                    Allocation {
                        task_id: other,
                        weight: 1.0,
                    },
                ],
            },
        )
        .unwrap();
        let day = get_day(&conn, "2030-01-10").unwrap();
        let s: Vec<_> = day
            .suggestions
            .iter()
            .map(|s| (s.task_id, s.reason.as_str(), s.always_suggested))
            .collect();
        assert_eq!(s, [(planned, "recorded", true), (other, "recorded", false)]);
    }

    #[test]
    fn suggestions_are_tasks_on_the_gantt_that_day_or_completed() {
        let conn = db::open_in_memory().unwrap();
        let doing = add(&conn, "作業中（日付なし）");
        let next = add(&conn, "次の予定だけ");
        let planned = add(&conn, "予定期間");
        let end_only = add(&conn, "終了予定日だけ");
        let outside = add(&conn, "期間外");
        let set = |id, json: &str| {
            let p: TaskPatch = serde_json::from_str(json).unwrap();
            ts::update_task(&conn, id, p).unwrap();
        };
        set(doing, r#"{"status":"doing"}"#);
        set(next, r#"{"next_action_date":"2030-01-10"}"#);
        set(
            planned,
            r#"{"planned_start":"2030-01-08","planned_end":"2030-01-12"}"#,
        );
        set(end_only, r#"{"planned_end":"2030-01-10"}"#);
        set(
            outside,
            r#"{"planned_start":"2030-01-11","planned_end":"2030-01-12"}"#,
        );
        let day = get_day(&conn, "2030-01-10").unwrap();
        let s: Vec<_> = day
            .suggestions
            .iter()
            .map(|s| (s.task_id, s.reason.as_str()))
            .collect();
        // 作業中・次の予定だけのもの・期間外は出さない
        assert_eq!(s, [(planned, "planned"), (end_only, "planned")]);

        // その日に完了したものは出す
        set(doing, r#"{"status":"done"}"#);
        let today = crate::service::today().to_string();
        let day = get_day(&conn, &today).unwrap();
        assert!(
            day.suggestions
                .iter()
                .any(|s| s.task_id == doing && s.reason == SuggestReason::Completed)
        );
    }

    #[test]
    fn save_day_can_be_undone() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a");
        let input = SaveDay {
            worked_min: 480,
            allocations: vec![Allocation {
                task_id: a,
                weight: 1.0,
            }],
        };
        undo_service::record(&conn, "工数", |c| save_day(c, "2026-11-20", input)).unwrap();
        assert_eq!(ts::get_task(&conn, a).unwrap().actual_min, 480);
        undo_service::undo(&conn).unwrap();
        assert_eq!(ts::get_task(&conn, a).unwrap().actual_min, 0);
        assert_eq!(get_day(&conn, "2026-11-20").unwrap().worked_min, None);
        undo_service::redo(&conn).unwrap();
        assert_eq!(get_day(&conn, "2026-11-20").unwrap().worked_min, Some(480));
    }

    #[test]
    fn settings_update_and_validate() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(get_settings(&conn).unwrap().minutes_per_day, 480);
        let s = update_settings(
            &conn,
            SettingsPatch {
                minutes_per_day: Some(450),
                default_work_min: Some(450),
            },
        )
        .unwrap();
        assert_eq!((s.minutes_per_day, s.default_work_min), (450, 450));
        assert!(
            update_settings(
                &conn,
                SettingsPatch {
                    default_work_min: Some(445),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
