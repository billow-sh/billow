use super::types::{
    ActualState, CleanupState, DesiredState, Workload, WorkloadError, WorkloadKind, WorkloadResult,
    WorkloadRun, now_unix_secs,
};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) const CLEANUP_RETRY_BACKOFF_SECS: i64 = 30;
pub(crate) const CLEANUP_RETRY_BACKOFF_CAP_SECS: i64 = 300;

#[derive(Clone)]
pub(crate) struct WorkloadStorage {
    connection: Arc<Mutex<Connection>>,
}

impl WorkloadStorage {
    pub(crate) fn open(path: impl AsRef<Path>) -> WorkloadResult<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    WorkloadError::internal(format!(
                        "failed to create workload database directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
        }

        let connection = Connection::open(path.as_ref()).map_err(sql_error)?;
        Self::from_connection(connection)
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory() -> WorkloadResult<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(sql_error)?)
    }

    fn from_connection(connection: Connection) -> WorkloadResult<Self> {
        let storage = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        storage.initialize()?;
        Ok(storage)
    }

    fn initialize(&self) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute_batch(
                "\
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS workloads (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    image TEXT NOT NULL,
                    desired_state TEXT NOT NULL,
                    actual_state TEXT NOT NULL,
                    exit_code INTEGER,
                    error TEXT,
                    stopping_since_unix_secs INTEGER,
                    runtime_unknown_since_unix_secs INTEGER,
                    restart_attempts INTEGER NOT NULL DEFAULT 0,
                    restart_not_before_unix_secs INTEGER,
                    running_since_unix_secs INTEGER,
                    created_at_unix_secs INTEGER NOT NULL,
                    updated_at_unix_secs INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS workload_runs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    workload_id TEXT NOT NULL REFERENCES workloads(id) ON DELETE CASCADE,
                    runtime_task_id TEXT NOT NULL UNIQUE,
                    cleanup_state TEXT NOT NULL DEFAULT 'pending',
                    cleanup_attempts INTEGER NOT NULL DEFAULT 0,
                    last_cleanup_error TEXT,
                    created_at_unix_secs INTEGER NOT NULL,
                    updated_at_unix_secs INTEGER NOT NULL
                );
                ",
            )
            .map_err(sql_error)?;
        Ok(())
    }

    pub(crate) fn save(&self, workload: &Workload) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                INSERT INTO workloads (
                    id, kind, image, desired_state, actual_state, exit_code, error,
                    stopping_since_unix_secs, runtime_unknown_since_unix_secs, restart_attempts,
                    restart_not_before_unix_secs, running_since_unix_secs, created_at_unix_secs,
                    updated_at_unix_secs
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ",
                params![
                    &workload.id,
                    workload.kind.as_str(),
                    &workload.image,
                    workload.desired_state.as_str(),
                    workload.actual_state.as_str(),
                    workload.exit_code,
                    &workload.error,
                    workload.stopping_since_unix_secs,
                    workload.runtime_unknown_since_unix_secs,
                    workload.restart_attempts,
                    workload.restart_not_before_unix_secs,
                    workload.running_since_unix_secs,
                    workload.created_at_unix_secs,
                    workload.updated_at_unix_secs,
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    pub(crate) fn get(&self, workload_id: &str) -> WorkloadResult<Workload> {
        let connection = self.connection()?;
        connection
            .query_row(
                workload_query("WHERE w.id = ?1").as_str(),
                [workload_id],
                workload_from_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| WorkloadError::not_found(format!("workload {workload_id} not found")))
    }

    pub(crate) fn list_for_reconcile(&self) -> WorkloadResult<Vec<Workload>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(workload_query("WHERE w.actual_state != ?1").as_str())
            .map_err(sql_error)?;
        let workloads = statement
            .query_map(params![ActualState::Deleted.as_str()], workload_from_row)
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        Ok(workloads)
    }

    pub(crate) fn list_watchable(&self) -> WorkloadResult<Vec<Workload>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(workload_query("WHERE w.actual_state IN (?1, ?2, ?3)").as_str())
            .map_err(sql_error)?;
        let workloads = statement
            .query_map(
                params![
                    ActualState::Starting.as_str(),
                    ActualState::Running.as_str(),
                    ActualState::Stopping.as_str(),
                ],
                workload_from_row,
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        Ok(workloads)
    }

    pub(crate) fn set_desired(
        &self,
        workload_id: &str,
        desired_state: DesiredState,
    ) -> WorkloadResult<()> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET desired_state = ?2, updated_at_unix_secs = ?3
                WHERE id = ?1
                ",
                params![workload_id, desired_state.as_str(), updated_at],
            )
            .map_err(sql_error)?;
        ensure_changed(workload_id, changed)
    }

    pub(crate) fn set_desired_unless_deleted(
        &self,
        workload_id: &str,
        desired_state: DesiredState,
    ) -> WorkloadResult<bool> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET desired_state = ?2, updated_at_unix_secs = ?3
                WHERE id = ?1
                  AND desired_state != ?4
                  AND actual_state != ?5
                ",
                params![
                    workload_id,
                    desired_state.as_str(),
                    updated_at,
                    DesiredState::Deleted.as_str(),
                    ActualState::Deleted.as_str(),
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn compare_and_set_actual(
        &self,
        workload_id: &str,
        expected_state: ActualState,
        actual_state: ActualState,
        exit_code: Option<u32>,
        error: Option<&str>,
    ) -> WorkloadResult<bool> {
        let updated_at = now_unix_secs();
        let clear_stopping_since =
            expected_state == ActualState::Stopping && actual_state != ActualState::Stopping;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET actual_state = ?2,
                    exit_code = ?3,
                    error = ?4,
                    stopping_since_unix_secs = CASE
                        WHEN ?2 = ?9 THEN COALESCE(stopping_since_unix_secs, ?6)
                        WHEN ?7 != 0 THEN NULL
                        ELSE stopping_since_unix_secs
                    END,
                    runtime_unknown_since_unix_secs = NULL,
                    running_since_unix_secs = CASE
                        WHEN ?2 = ?10 THEN COALESCE(running_since_unix_secs, ?5)
                        ELSE NULL
                    END,
                    updated_at_unix_secs = ?5
                WHERE id = ?1 AND actual_state = ?8
                ",
                params![
                    workload_id,
                    actual_state.as_str(),
                    exit_code,
                    error,
                    updated_at,
                    updated_at,
                    if clear_stopping_since { 1 } else { 0 },
                    expected_state.as_str(),
                    ActualState::Stopping.as_str(),
                    ActualState::Running.as_str(),
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn set_stopped_outcome(
        &self,
        workload_id: &str,
        expected_state: ActualState,
        exit_code: Option<u32>,
    ) -> WorkloadResult<bool> {
        let updated_at = now_unix_secs();
        let clear_stopping_since = expected_state == ActualState::Stopping;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET actual_state = CASE
                        WHEN desired_state IN (?6, ?7) THEN ?8
                        WHEN ?2 IS NOT NULL AND ?2 = 0 THEN ?8
                        ELSE ?9
                    END,
                    exit_code = CASE
                        WHEN desired_state IN (?6, ?7) THEN NULL
                        ELSE ?2
                    END,
                    error = CASE
                        WHEN desired_state IN (?6, ?7) THEN NULL
                        WHEN ?2 IS NOT NULL AND ?2 = 0 THEN NULL
                        WHEN ?2 IS NOT NULL THEN 'runtime task exited with non-zero status'
                        ELSE 'runtime task exited without an exit code'
                    END,
                    stopping_since_unix_secs = CASE
                        WHEN ?5 != 0 THEN NULL
                        ELSE stopping_since_unix_secs
                    END,
                    runtime_unknown_since_unix_secs = NULL,
                    running_since_unix_secs = NULL,
                    updated_at_unix_secs = ?3
                WHERE id = ?1 AND actual_state = ?4
                ",
                params![
                    workload_id,
                    exit_code,
                    updated_at,
                    expected_state.as_str(),
                    if clear_stopping_since { 1 } else { 0 },
                    DesiredState::Stopped.as_str(),
                    DesiredState::Deleted.as_str(),
                    ActualState::Stopped.as_str(),
                    ActualState::Failed.as_str(),
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn mark_runtime_unknown(
        &self,
        workload_id: &str,
        expected_state: ActualState,
        unknown_since_unix_secs: i64,
    ) -> WorkloadResult<bool> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET runtime_unknown_since_unix_secs = COALESCE(
                        runtime_unknown_since_unix_secs,
                        ?3
                    ),
                    updated_at_unix_secs = CASE
                        WHEN runtime_unknown_since_unix_secs IS NULL THEN ?3
                        ELSE updated_at_unix_secs
                    END
                WHERE id = ?1 AND actual_state = ?2
                ",
                params![
                    workload_id,
                    expected_state.as_str(),
                    unknown_since_unix_secs
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn set_restart_backoff(
        &self,
        workload_id: &str,
        expected_state: ActualState,
        restart_attempts: i64,
        restart_not_before_unix_secs: i64,
    ) -> WorkloadResult<bool> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET restart_attempts = ?3,
                    restart_not_before_unix_secs = ?4,
                    updated_at_unix_secs = ?5
                WHERE id = ?1 AND actual_state = ?2 AND desired_state = ?6
                ",
                params![
                    workload_id,
                    expected_state.as_str(),
                    restart_attempts,
                    restart_not_before_unix_secs,
                    updated_at,
                    DesiredState::Running.as_str(),
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn clear_restart_backoff(&self, workload_id: &str) -> WorkloadResult<()> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workloads
                SET restart_attempts = 0,
                    restart_not_before_unix_secs = NULL,
                    updated_at_unix_secs = ?2
                WHERE id = ?1
                ",
                params![workload_id, updated_at],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    pub(crate) fn set_desired_running_unless_deleted_and_clear_backoff(
        &self,
        workload_id: &str,
    ) -> WorkloadResult<bool> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "\
                UPDATE workloads
                SET desired_state = ?3,
                    restart_attempts = 0,
                    restart_not_before_unix_secs = NULL,
                    updated_at_unix_secs = ?2
                WHERE id = ?1
                  AND desired_state != ?4
                  AND actual_state != ?5
                ",
                params![
                    workload_id,
                    updated_at,
                    DesiredState::Running.as_str(),
                    DesiredState::Deleted.as_str(),
                    ActualState::Deleted.as_str(),
                ],
            )
            .map_err(sql_error)?;
        Ok(changed != 0)
    }

    pub(crate) fn begin_start(
        &self,
        workload_id: &str,
        expected_state: ActualState,
        runtime_task_id: &str,
    ) -> WorkloadResult<bool> {
        let now = now_unix_secs();
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(sql_error)?;
        let changed = transaction
            .execute(
                "\
                UPDATE workloads
                SET actual_state = ?2,
                    exit_code = NULL,
                    error = NULL,
                    stopping_since_unix_secs = NULL,
                    runtime_unknown_since_unix_secs = NULL,
                    restart_not_before_unix_secs = NULL,
                    running_since_unix_secs = NULL,
                    updated_at_unix_secs = ?3
                WHERE id = ?1 AND actual_state = ?4 AND desired_state = ?5
                ",
                params![
                    workload_id,
                    ActualState::Creating.as_str(),
                    now,
                    expected_state.as_str(),
                    DesiredState::Running.as_str(),
                ],
            )
            .map_err(sql_error)?;
        if changed == 0 {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }

        transaction
            .execute(
                "\
                INSERT INTO workload_runs (
                    workload_id, runtime_task_id, cleanup_state, cleanup_attempts,
                    last_cleanup_error, created_at_unix_secs, updated_at_unix_secs
                )
                VALUES (?1, ?2, 'pending', 0, NULL, ?3, ?4)
                ",
                params![workload_id, runtime_task_id, now, now],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }

    pub(crate) fn mark_run_cleanup_done(&self, runtime_task_id: &str) -> WorkloadResult<()> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workload_runs
                SET cleanup_state = 'done',
                    last_cleanup_error = NULL,
                    updated_at_unix_secs = ?2
                WHERE runtime_task_id = ?1
                ",
                params![runtime_task_id, updated_at],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    pub(crate) fn record_run_cleanup_failure(
        &self,
        runtime_task_id: &str,
        cleanup_state: CleanupState,
        cleanup_attempts: i64,
        error: &str,
    ) -> WorkloadResult<()> {
        let updated_at = now_unix_secs();
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workload_runs
                SET cleanup_state = ?2,
                    cleanup_attempts = ?3,
                    last_cleanup_error = ?4,
                    updated_at_unix_secs = ?5
                WHERE runtime_task_id = ?1
                ",
                params![
                    runtime_task_id,
                    cleanup_state.as_str(),
                    cleanup_attempts,
                    error,
                    updated_at
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    pub(crate) fn latest_run(&self, workload_id: &str) -> WorkloadResult<Option<WorkloadRun>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "\
                SELECT
                    workload_id,
                    runtime_task_id,
                    cleanup_state,
                    cleanup_attempts,
                    last_cleanup_error,
                    updated_at_unix_secs
                FROM workload_runs
                WHERE workload_id = ?1
                ORDER BY id DESC
                LIMIT 1
                ",
                [workload_id],
                run_from_row,
            )
            .optional()
            .map_err(sql_error)
    }

    pub(crate) fn list_prunable_runs(&self) -> WorkloadResult<Vec<WorkloadRun>> {
        let now = now_unix_secs();
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "\
                SELECT
                    workload_id,
                    runtime_task_id,
                    cleanup_state,
                    cleanup_attempts,
                    last_cleanup_error,
                    updated_at_unix_secs
                FROM workload_runs
                WHERE id NOT IN (
                    SELECT MAX(id)
                    FROM workload_runs
                    GROUP BY workload_id
                  )
                  AND (
                    cleanup_state = ?4
                    OR ?1 - updated_at_unix_secs >= min(?2 * cleanup_attempts, ?3)
                  )
                ORDER BY id
                ",
            )
            .map_err(sql_error)?;
        let runs = statement
            .query_map(
                params![
                    now,
                    CLEANUP_RETRY_BACKOFF_SECS,
                    CLEANUP_RETRY_BACKOFF_CAP_SECS,
                    CleanupState::Done.as_str(),
                ],
                run_from_row,
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        Ok(runs)
    }

    pub(crate) fn delete_run(&self, runtime_task_id: &str) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM workload_runs WHERE runtime_task_id = ?1",
                [runtime_task_id],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_stopping_since_for_test(
        &self,
        workload_id: &str,
        stopping_since_unix_secs: i64,
    ) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workloads
                SET stopping_since_unix_secs = ?2
                WHERE id = ?1
                ",
                params![workload_id, stopping_since_unix_secs],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn count_runs_for_test(&self, workload_id: &str) -> WorkloadResult<i64> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT COUNT(*) FROM workload_runs WHERE workload_id = ?1",
                [workload_id],
                |row| row.get(0),
            )
            .map_err(sql_error)
    }

    #[cfg(test)]
    pub(crate) fn backdate_run_for_test(
        &self,
        runtime_task_id: &str,
        updated_at_unix_secs: i64,
    ) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workload_runs
                SET updated_at_unix_secs = ?2
                WHERE runtime_task_id = ?1
                ",
                params![runtime_task_id, updated_at_unix_secs],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_unknown_since_for_test(
        &self,
        workload_id: &str,
        runtime_unknown_since_unix_secs: i64,
    ) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workloads
                SET runtime_unknown_since_unix_secs = ?2
                WHERE id = ?1
                ",
                params![workload_id, runtime_unknown_since_unix_secs],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_restart_backoff_for_test(
        &self,
        workload_id: &str,
        restart_attempts: i64,
        restart_not_before_unix_secs: Option<i64>,
    ) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workloads
                SET restart_attempts = ?2,
                    restart_not_before_unix_secs = ?3
                WHERE id = ?1
                ",
                params![workload_id, restart_attempts, restart_not_before_unix_secs],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_running_since_for_test(
        &self,
        workload_id: &str,
        running_since_unix_secs: i64,
    ) -> WorkloadResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "\
                UPDATE workloads
                SET running_since_unix_secs = ?2
                WHERE id = ?1
                ",
                params![workload_id, running_since_unix_secs],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn connection(&self) -> WorkloadResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| WorkloadError::internal("workload storage lock is poisoned"))
    }
}

fn workload_query(predicate: &str) -> String {
    format!(
        "\
        SELECT
            w.id,
            w.kind,
            w.image,
            w.desired_state,
            w.actual_state,
            latest_run.runtime_task_id,
            w.exit_code,
            w.error,
            w.stopping_since_unix_secs,
            w.runtime_unknown_since_unix_secs,
            w.restart_attempts,
            w.restart_not_before_unix_secs,
            w.running_since_unix_secs,
            w.created_at_unix_secs,
            w.updated_at_unix_secs
        FROM workloads w
        LEFT JOIN (
            SELECT r.workload_id, r.runtime_task_id
            FROM workload_runs r
            JOIN (
                SELECT workload_id, MAX(id) AS max_id
                FROM workload_runs
                GROUP BY workload_id
            ) m ON m.workload_id = r.workload_id AND m.max_id = r.id
        ) latest_run ON latest_run.workload_id = w.id
        {predicate}
        "
    )
}

fn workload_from_row(row: &Row<'_>) -> rusqlite::Result<Workload> {
    let kind: String = row.get(1)?;
    let desired_state: String = row.get(3)?;
    let actual_state: String = row.get(4)?;
    Ok(Workload {
        id: row.get(0)?,
        kind: WorkloadKind::from_str(&kind).ok_or_else(|| invalid_text("kind", &kind))?,
        image: row.get(2)?,
        desired_state: DesiredState::from_str(&desired_state)
            .ok_or_else(|| invalid_text("desired_state", &desired_state))?,
        actual_state: ActualState::from_str(&actual_state)
            .ok_or_else(|| invalid_text("actual_state", &actual_state))?,
        runtime_task_id: row.get(5)?,
        exit_code: row.get(6)?,
        error: row.get(7)?,
        stopping_since_unix_secs: row.get(8)?,
        runtime_unknown_since_unix_secs: row.get(9)?,
        restart_attempts: row.get(10)?,
        restart_not_before_unix_secs: row.get(11)?,
        running_since_unix_secs: row.get(12)?,
        created_at_unix_secs: row.get(13)?,
        updated_at_unix_secs: row.get(14)?,
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<WorkloadRun> {
    let cleanup_state: String = row.get(2)?;
    Ok(WorkloadRun {
        workload_id: row.get(0)?,
        runtime_task_id: row.get(1)?,
        cleanup_state: CleanupState::from_str(&cleanup_state)
            .ok_or_else(|| invalid_text("cleanup_state", &cleanup_state))?,
        cleanup_attempts: row.get(3)?,
        last_cleanup_error: row.get(4)?,
        updated_at_unix_secs: row.get(5)?,
    })
}

fn invalid_text(column: &'static str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        Type::Text,
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {column} value {value:?}"),
        )),
    )
}

fn ensure_changed(workload_id: &str, changed: usize) -> WorkloadResult<()> {
    if changed == 0 {
        Err(WorkloadError::not_found(format!(
            "workload {workload_id} not found"
        )))
    } else {
        Ok(())
    }
}

fn sql_error(error: rusqlite::Error) -> WorkloadError {
    WorkloadError::internal(error.to_string())
}
