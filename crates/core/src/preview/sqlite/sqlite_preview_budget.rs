//! Execution limits for SQL read from evidence, including recursive views.
//! A row LIMIT does not bound work done by a view, COUNT or ORDER BY.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::Connection;

const CHECK_INTERVAL: i32 = 1_000;
const MAX_VM_STEPS: u64 = 50_000_000;
const MAX_DURATION: Duration = Duration::from_secs(5);

/// Bound one complete preview request, sharing the budget across statements.
/// Always remove the hook afterwards so the connection can be reused.
pub(crate) fn run<T>(
    connection: &Connection,
    query: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    run_with_limits(connection, MAX_VM_STEPS, MAX_DURATION, query)
}

fn run_with_limits<T>(
    connection: &Connection,
    max_steps: u64,
    max_duration: Duration,
    query: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let started = Instant::now();
    let exceeded = Arc::new(AtomicBool::new(false));
    let mut remaining = max_steps;
    connection.progress_handler(
        CHECK_INTERVAL,
        Some({
            let exceeded = Arc::clone(&exceeded);
            move || {
                remaining = remaining.saturating_sub(CHECK_INTERVAL as u64);
                let stop = remaining == 0 || started.elapsed() >= max_duration;
                if stop {
                    exceeded.store(true, Ordering::Relaxed);
                }
                stop
            }
        }),
    )?;
    let result = query();
    connection.progress_handler(0, None::<fn() -> bool>)?;
    // Some cosmetic queries tolerate errors. Budget exhaustion must never
    // become an empty table or a successful, incomplete preview.
    if exceeded.load(Ordering::Relaxed) || started.elapsed() >= max_duration {
        anyhow::bail!("SQLite 预览查询超过执行限制，已停止本次查询");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupts_recursive_view_despite_row_limit_and_releases_connection() {
        let connection = Connection::open_in_memory().expect("open");
        connection
            .execute_batch(
                "CREATE VIEW endless AS WITH RECURSIVE numbers(value) AS (
                 SELECT 1 UNION ALL SELECT value + 1 FROM numbers
             ) SELECT value FROM numbers ORDER BY value DESC;",
            )
            .expect("view");
        for sql in [
            "SELECT * FROM endless LIMIT 20",
            "SELECT COUNT(*) FROM endless",
        ] {
            let error = run_with_limits(&connection, 10_000, Duration::from_secs(5), || {
                Ok(connection.query_row(sql, [], |row| row.get::<_, i64>(0))?)
            })
            .expect_err("recursive query must be interrupted");
            assert!(error.to_string().contains("超过执行限制"));
            assert_eq!(
                connection
                    .query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                    .expect("usable"),
                42,
            );
        }
    }

    #[test]
    fn swallowed_interrupt_is_still_reported_and_budget_is_shared() {
        let connection = Connection::open_in_memory().expect("open");
        let error =
            run_with_limits(&connection, 10_000, Duration::from_secs(5), || {
                for _ in 0..100 {
                    if connection.query_row(
                    "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<100)
                     SELECT SUM(x) FROM n", [], |row| row.get::<_, i64>(0),
                ).is_err() {
                    break;
                }
                }
                Ok(())
            })
            .expect_err("all statements share the request budget");
        assert!(error.to_string().contains("超过执行限制"));
    }

    #[test]
    fn elapsed_deadline_is_reported() {
        let connection = Connection::open_in_memory().expect("open");
        assert!(run_with_limits(&connection, u64::MAX, Duration::ZERO, || Ok(())).is_err());
    }
}
