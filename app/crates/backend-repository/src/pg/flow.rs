use crate::traits::*;
use backend_core::{Error, async_trait};
use backend_model::db;
use chrono::{DateTime, Utc};
use diesel::dsl::count_star;
use diesel::prelude::*;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::deadpool::Pool;
use serde_json::Value;
use tracing::{debug, instrument};

#[derive(Clone)]
pub struct FlowRepository {
    pub(crate) pool: Pool<AsyncPgConnection>,
}

impl FlowRepository {
    pub fn new(pool: Pool<AsyncPgConnection>) -> Self {
        Self { pool }
    }

    async fn get_conn(
        &self,
    ) -> RepoResult<diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>> {
        self.pool
            .get()
            .await
            .map_err(|error| Error::DieselPool(error.to_string()))
    }
}

#[async_trait]
impl FlowRepo for FlowRepository {
    #[instrument(skip(self))]
    async fn create_session(
        &self,
        input: FlowSessionCreateInput,
    ) -> RepoResult<db::FlowSessionRow> {
        debug!("Creating session: {}", input.session_type);
        use backend_model::schema::flow_session;

        let mut conn = self.get_conn().await?;
        let now = Utc::now();
        let row = db::FlowSessionRow {
            id: input.id,
            human_id: input.human_id,
            user_id: input.user_id,
            session_type: input.session_type,
            status: input.status,
            context: input.context,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };

        match diesel::insert_into(flow_session::table)
            .values(&row)
            .get_result::<db::FlowSessionRow>(&mut conn)
            .await
        {
            Ok(created) => Ok(created),
            Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
                flow_session::table
                    .filter(flow_session::human_id.eq(&row.human_id))
                    .select(db::FlowSessionRow::as_select())
                    .first::<db::FlowSessionRow>(&mut conn)
                    .await
                    .optional()
                    .map_err(Error::from)?
                    .ok_or_else(|| {
                        Error::conflict("FLOW_SESSION_CONFLICT", "Session already exists")
                    })
            }
            Err(error) => Err(error.into()),
        }
    }

    #[instrument(skip(self))]
    async fn get_session(&self, session_id: &str) -> RepoResult<Option<db::FlowSessionRow>> {
        debug!("Getting session: {}", session_id);
        use backend_model::schema::flow_session;

        let mut conn = self.get_conn().await?;
        flow_session::table
            .filter(flow_session::id.eq(session_id))
            .select(db::FlowSessionRow::as_select())
            .first::<db::FlowSessionRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn list_sessions(
        &self,
        filter: FlowSessionFilter,
    ) -> RepoResult<(Vec<db::FlowSessionRow>, i64)> {
        debug!("Listing sessions with filter: {:?}", filter);
        use backend_model::schema::flow_session;

        let filter = filter.normalized();
        let mut conn = self.get_conn().await?;

        let mut count_query = flow_session::table.into_boxed();
        let mut rows_query = flow_session::table.into_boxed();

        if let Some(user_id) = filter.user_id.as_ref() {
            count_query = count_query.filter(flow_session::user_id.eq(user_id));
            rows_query = rows_query.filter(flow_session::user_id.eq(user_id));
        }
        if let Some(user_ids) = filter.user_ids.as_ref() {
            let nullable_ids: Vec<Option<String>> = user_ids.iter().cloned().map(Some).collect();
            count_query = count_query.filter(flow_session::user_id.eq_any(nullable_ids.clone()));
            rows_query = rows_query.filter(flow_session::user_id.eq_any(nullable_ids));
        }
        if let Some(session_type) = filter.session_type.as_ref() {
            count_query = count_query.filter(flow_session::session_type.eq(session_type));
            rows_query = rows_query.filter(flow_session::session_type.eq(session_type));
        }
        if let Some(status) = filter.status.as_ref() {
            count_query = count_query.filter(flow_session::status.eq(status));
            rows_query = rows_query.filter(flow_session::status.eq(status));
        }

        let total = count_query
            .select(count_star())
            .get_result::<i64>(&mut conn)
            .await
            .map_err(Error::from)?;

        let rows = rows_query
            .order(flow_session::updated_at.desc())
            .limit(i64::from(filter.limit))
            .offset(filter.offset())
            .select(db::FlowSessionRow::as_select())
            .load::<db::FlowSessionRow>(&mut conn)
            .await
            .map_err(Error::from)?;

        Ok((rows, total))
    }

    #[instrument(skip(self))]
    async fn update_session_status(
        &self,
        session_id: &str,
        status: &str,
        completed_at: Option<DateTime<Utc>>,
    ) -> RepoResult<()> {
        debug!("Updating session status: {} -> {}", session_id, status);
        use backend_model::schema::flow_session;

        let mut conn = self.get_conn().await?;
        diesel::update(flow_session::table.filter(flow_session::id.eq(session_id)))
            .set((
                flow_session::status.eq(status),
                flow_session::updated_at.eq(Utc::now()),
                flow_session::completed_at.eq(completed_at),
            ))
            .execute(&mut conn)
            .await
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self, context))]
    async fn update_session_context(&self, session_id: &str, context: Value) -> RepoResult<()> {
        debug!("Updating session context: {}", session_id);
        use backend_model::schema::flow_session;

        let mut conn = self.get_conn().await?;
        diesel::update(flow_session::table.filter(flow_session::id.eq(session_id)))
            .set((
                flow_session::context.eq(context),
                flow_session::updated_at.eq(Utc::now()),
            ))
            .execute(&mut conn)
            .await
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn create_flow(&self, input: FlowInstanceCreateInput) -> RepoResult<db::FlowInstanceRow> {
        debug!("Creating flow instance: {}", input.flow_type);
        use backend_model::schema::flow_instance;

        let mut conn = self.get_conn().await?;
        let now = Utc::now();
        let row = db::FlowInstanceRow {
            id: input.id,
            human_id: input.human_id,
            session_id: input.session_id,
            flow_type: input.flow_type,
            status: input.status,
            current_step: input.current_step,
            step_ids: input.step_ids,
            context: input.context,
            created_at: now,
            updated_at: now,
        };

        match diesel::insert_into(flow_instance::table)
            .values(&row)
            .get_result::<db::FlowInstanceRow>(&mut conn)
            .await
        {
            Ok(created) => Ok(created),
            Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
                flow_instance::table
                    .filter(flow_instance::human_id.eq(&row.human_id))
                    .select(db::FlowInstanceRow::as_select())
                    .first::<db::FlowInstanceRow>(&mut conn)
                    .await
                    .optional()
                    .map_err(Error::from)?
                    .ok_or_else(|| Error::conflict("FLOW_INSTANCE_CONFLICT", "Flow already exists"))
            }
            Err(error) => Err(error.into()),
        }
    }

    #[instrument(skip(self))]
    async fn get_flow(&self, flow_id: &str) -> RepoResult<Option<db::FlowInstanceRow>> {
        debug!("Getting flow: {}", flow_id);
        use backend_model::schema::flow_instance;

        let mut conn = self.get_conn().await?;
        flow_instance::table
            .filter(flow_instance::id.eq(flow_id))
            .select(db::FlowInstanceRow::as_select())
            .first::<db::FlowInstanceRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn list_flows_for_session(
        &self,
        session_id: &str,
    ) -> RepoResult<Vec<db::FlowInstanceRow>> {
        debug!("Listing flows for session: {}", session_id);
        use backend_model::schema::flow_instance;

        let mut conn = self.get_conn().await?;
        flow_instance::table
            .filter(flow_instance::session_id.eq(session_id))
            .order(flow_instance::created_at.asc())
            .select(db::FlowInstanceRow::as_select())
            .load::<db::FlowInstanceRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self, step_ids, context))]
    async fn update_flow(
        &self,
        flow_id: &str,
        status: Option<String>,
        current_step: Option<Option<String>>,
        step_ids: Option<Value>,
        context: Option<Value>,
    ) -> RepoResult<db::FlowInstanceRow> {
        debug!("Updating flow: {}", flow_id);
        use backend_model::schema::flow_instance;

        let mut conn = self.get_conn().await?;
        let current = flow_instance::table
            .filter(flow_instance::id.eq(flow_id))
            .select(db::FlowInstanceRow::as_select())
            .first::<db::FlowInstanceRow>(&mut conn)
            .await
            .optional()
            .map_err(Error::from)?
            .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

        let updated = db::FlowInstanceRow {
            status: status.unwrap_or_else(|| current.status.clone()),
            current_step: current_step.unwrap_or_else(|| current.current_step.clone()),
            step_ids: step_ids.unwrap_or_else(|| current.step_ids.clone()),
            context: context.unwrap_or_else(|| current.context.clone()),
            updated_at: Utc::now(),
            ..current
        };

        diesel::update(flow_instance::table.filter(flow_instance::id.eq(flow_id)))
            .set((
                flow_instance::status.eq(&updated.status),
                flow_instance::current_step.eq(&updated.current_step),
                flow_instance::step_ids.eq(&updated.step_ids),
                flow_instance::context.eq(&updated.context),
                flow_instance::updated_at.eq(updated.updated_at),
            ))
            .get_result::<db::FlowInstanceRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn create_step(&self, input: FlowStepCreateInput) -> RepoResult<db::FlowStepRow> {
        debug!(
            "Creating step: {} for flow: {}",
            input.step_type, input.flow_id
        );
        use backend_model::schema::flow_step;

        let mut conn = self.get_conn().await?;
        let now = Utc::now();
        let row = db::FlowStepRow {
            id: input.id,
            human_id: input.human_id,
            flow_id: input.flow_id,
            step_type: input.step_type,
            actor: input.actor,
            status: input.status,
            attempt_no: input.attempt_no,
            input: input.input,
            output: input.output,
            error: input.error,
            next_retry_at: input.next_retry_at,
            created_at: now,
            updated_at: now,
            finished_at: input.finished_at,
        };

        match diesel::insert_into(flow_step::table)
            .values(&row)
            .get_result::<db::FlowStepRow>(&mut conn)
            .await
        {
            Ok(created) => Ok(created),
            Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
                flow_step::table
                    .filter(flow_step::human_id.eq(&row.human_id))
                    .select(db::FlowStepRow::as_select())
                    .first::<db::FlowStepRow>(&mut conn)
                    .await
                    .optional()
                    .map_err(Error::from)?
                    .ok_or_else(|| Error::conflict("FLOW_STEP_CONFLICT", "Step already exists"))
            }
            Err(error) => Err(error.into()),
        }
    }

    #[instrument(skip(self))]
    async fn get_step(&self, step_id: &str) -> RepoResult<Option<db::FlowStepRow>> {
        debug!("Getting step: {}", step_id);
        use backend_model::schema::flow_step;

        let mut conn = self.get_conn().await?;
        flow_step::table
            .filter(flow_step::id.eq(step_id))
            .select(db::FlowStepRow::as_select())
            .first::<db::FlowStepRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn list_steps_for_flow(&self, flow_id: &str) -> RepoResult<Vec<db::FlowStepRow>> {
        debug!("Listing steps for flow: {}", flow_id);
        use backend_model::schema::flow_step;

        let mut conn = self.get_conn().await?;
        flow_step::table
            .filter(flow_step::flow_id.eq(flow_id))
            .order(flow_step::created_at.asc())
            .select(db::FlowStepRow::as_select())
            .load::<db::FlowStepRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self, patch))]
    async fn patch_step(&self, step_id: &str, patch: FlowStepPatch) -> RepoResult<db::FlowStepRow> {
        debug!("Patching step: {}", step_id);
        use backend_model::schema::flow_step;

        let mut conn = self.get_conn().await?;
        let current = flow_step::table
            .filter(flow_step::id.eq(step_id))
            .select(db::FlowStepRow::as_select())
            .first::<db::FlowStepRow>(&mut conn)
            .await
            .optional()
            .map_err(Error::from)?
            .ok_or_else(|| Error::not_found("FLOW_STEP_NOT_FOUND", "Step not found"))?;

        let updated = db::FlowStepRow {
            status: patch.status.unwrap_or_else(|| current.status.clone()),
            attempt_no: patch.attempt_no.unwrap_or(current.attempt_no),
            input: patch.input.unwrap_or_else(|| current.input.clone()),
            output: patch.output.unwrap_or_else(|| current.output.clone()),
            error: patch.error.unwrap_or_else(|| current.error.clone()),
            next_retry_at: patch.next_retry_at.unwrap_or(current.next_retry_at),
            finished_at: patch.finished_at.unwrap_or(current.finished_at),
            updated_at: Utc::now(),
            ..current
        };

        diesel::update(flow_step::table.filter(flow_step::id.eq(step_id)))
            .set((
                flow_step::status.eq(&updated.status),
                flow_step::attempt_no.eq(&updated.attempt_no),
                flow_step::input.eq(&updated.input),
                flow_step::output.eq(&updated.output),
                flow_step::error.eq(&updated.error),
                flow_step::next_retry_at.eq(&updated.next_retry_at),
                flow_step::finished_at.eq(&updated.finished_at),
                flow_step::updated_at.eq(updated.updated_at),
            ))
            .get_result::<db::FlowStepRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn claim_next_system_step(&self) -> RepoResult<Option<db::FlowStepRow>> {
        debug!("Claiming next system step");

        let mut conn = self.get_conn().await?;

        // Find a step that is WAITING+retry_due, and is a SYSTEM step.
        // We update its status to RUNNING to claim it.
        // Initial SYSTEM execution is handled synchronously in request-time step chains.
        // diesel_async doesn't natively support UPDATE ... RETURNING with a complex WHERE
        // easily without raw SQL or subqueries if we want to grab just 1 row atomically.
        // Let's use a subquery approach with raw SQL.

        let sql = diesel::sql_query(
            r#"
            UPDATE flow_step
            SET status = 'RUNNING', updated_at = NOW()
            WHERE id = (
                SELECT id
                FROM flow_step
                WHERE actor = 'SYSTEM'
                  AND status = 'WAITING'
                  AND next_retry_at <= NOW()
                ORDER BY updated_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            RETURNING *;
            "#,
        );

        sql.get_result::<db::FlowStepRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }
}
