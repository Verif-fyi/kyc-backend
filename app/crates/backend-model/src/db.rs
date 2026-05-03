//! Database row types for Diesel ORM operations.
//!
//! Each struct corresponds to a table row and implements Diesel traits
//! for query, insert, and selection operations.

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde_json::Value;

/// Static deposit recipient entry synced from configuration.
/// Composite primary key: (provider, currency)
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::app_deposit_recipients)]
pub struct AppDepositRecipientRow {
    pub provider: String,
    pub full_name: String,
    pub phone_number: String,
    pub phone_regex: String,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User account data stored in app_user table.
/// Primary key: user_id (prefixed CUID like usr_*)
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::app_user)]
pub struct UserRow {
    pub user_id: String,
    pub username: String,
    pub full_name: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub phone_number: Option<String>,
    pub disabled: bool,
    pub attributes: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Additional dynamic user data stored in app_user_data table.
/// Composite primary key: (user_id, name, data_type)
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::app_user_data)]
pub struct UserDataRow {
    pub user_id: String,
    pub name: String,
    pub data_type: String,
    pub content: Value,
    pub eager_fetch: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Top-level flow session row.
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::flow_session)]
pub struct FlowSessionRow {
    pub id: String,
    pub human_id: String,
    pub user_id: Option<String>,
    pub session_type: String,
    pub status: String,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Flow execution row within a session.
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::flow_instance)]
pub struct FlowInstanceRow {
    pub id: String,
    pub human_id: String,
    pub session_id: String,
    pub flow_type: String,
    pub status: String,
    pub current_step: Option<String>,
    pub step_ids: Value,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Step execution row within a flow.
#[derive(Debug, Clone, Queryable, Selectable, Insertable, QueryableByName)]
#[diesel(table_name = crate::schema::flow_step)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct FlowStepRow {
    pub id: String,
    pub human_id: String,
    pub flow_id: String,
    pub step_type: String,
    pub actor: String,
    pub status: String,
    pub attempt_no: i32,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// State machine instance - represents a single KYC flow execution.
impl diesel::associations::HasTable for UserRow {
    type Table = crate::schema::app_user::table;

    fn table() -> Self::Table {
        crate::schema::app_user::table
    }
}

impl diesel::associations::HasTable for AppDepositRecipientRow {
    type Table = crate::schema::app_deposit_recipients::table;

    fn table() -> Self::Table {
        crate::schema::app_deposit_recipients::table
    }
}

impl diesel::associations::HasTable for UserDataRow {
    type Table = crate::schema::app_user_data::table;

    fn table() -> Self::Table {
        crate::schema::app_user_data::table
    }
}

impl diesel::associations::HasTable for FlowSessionRow {
    type Table = crate::schema::flow_session::table;

    fn table() -> Self::Table {
        crate::schema::flow_session::table
    }
}

impl diesel::associations::HasTable for FlowInstanceRow {
    type Table = crate::schema::flow_instance::table;

    fn table() -> Self::Table {
        crate::schema::flow_instance::table
    }
}

impl diesel::associations::HasTable for FlowStepRow {
    type Table = crate::schema::flow_step::table;

    fn table() -> Self::Table {
        crate::schema::flow_step::table
    }
}

impl<'a> diesel::Identifiable for &'a UserRow {
    type Id = &'a str;

    fn id(self) -> Self::Id {
        self.user_id.as_str()
    }
}

impl<'a> diesel::Identifiable for &'a AppDepositRecipientRow {
    type Id = (&'a str, &'a str);

    fn id(self) -> Self::Id {
        (self.provider.as_str(), self.currency.as_str())
    }
}

impl<'a> diesel::Identifiable for &'a UserDataRow {
    type Id = (&'a str, &'a str, &'a str);

    fn id(self) -> Self::Id {
        (
            self.user_id.as_str(),
            self.name.as_str(),
            self.data_type.as_str(),
        )
    }
}

impl<'a> diesel::Identifiable for &'a FlowSessionRow {
    type Id = &'a str;

    fn id(self) -> Self::Id {
        self.id.as_str()
    }
}

impl<'a> diesel::Identifiable for &'a FlowInstanceRow {
    type Id = &'a str;

    fn id(self) -> Self::Id {
        self.id.as_str()
    }
}

impl<'a> diesel::Identifiable for &'a FlowStepRow {
    type Id = &'a str;

    fn id(self) -> Self::Id {
        self.id.as_str()
    }
}
