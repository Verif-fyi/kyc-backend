//! Generic user data transfer objects.

use std::collections::HashMap;

/// User attributes map (string -> string)
pub type UserAttributes = HashMap<String, String>;

/// User upsert request
#[derive(Debug, Clone)]
pub struct UserUpsert {
    pub username: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub enabled: Option<bool>,
    pub email_verified: Option<bool>,
    pub attributes: Option<UserAttributes>,
}

/// User search request
#[derive(Debug, Clone)]
pub struct UserSearch {
    pub search: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub enabled: Option<bool>,
    pub email_verified: Option<bool>,
    pub exact: Option<bool>,
    pub attributes: Option<UserAttributes>,
    pub first_result: Option<i32>,
    pub max_results: Option<i32>,
}
