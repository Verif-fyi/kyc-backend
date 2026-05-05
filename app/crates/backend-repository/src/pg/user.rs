use crate::traits::*;
use backend_core::async_trait;
use backend_model::{db, user as user_dto};
use diesel::PgJsonbExpressionMethods;
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::deadpool::Pool;
use tracing::{debug, instrument};

#[derive(Clone)]
pub struct UserRepository {
    pub(crate) pool: Pool<AsyncPgConnection>,
}

const USER_METADATA_DATA_TYPE: &str = "metadata";

impl UserRepository {
    pub fn new(pool: Pool<AsyncPgConnection>) -> Self {
        Self { pool }
    }

    async fn get_conn(
        &self,
    ) -> RepoResult<diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>> {
        self.pool
            .get()
            .await
            .map_err(|e| backend_core::Error::DieselPool(e.to_string()))
    }
}

fn merge_json_value(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_obj), serde_json::Value::Object(patch_obj)) => {
            for (key, value) in patch_obj {
                if value.is_null() {
                    base_obj.remove(key);
                    continue;
                }

                if let Some(existing) = base_obj.get_mut(key) {
                    merge_json_value(existing, value);
                } else {
                    base_obj.insert(key.clone(), value.clone());
                }
            }
        }
        (slot, value) => {
            *slot = value.clone();
        }
    }
}

fn normalize_full_name(name: Option<String>) -> Option<String> {
    name.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

fn normalize_search_field(value: Option<&String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

#[async_trait]
impl UserRepo for UserRepository {
    #[instrument(skip(self))]
    async fn create_user(&self, req: &user_dto::UserUpsert) -> RepoResult<db::UserRow> {
        debug!("Creating user: {:?}", req.username);
        use backend_model::schema::app_user::dsl::*;

        let user_id_val = backend_core::id::user_id().map_err(|e| backend_core::Error::internal("ID_GEN", e))?;
        let mut conn = self.get_conn().await?;

        let attributes_json = req
            .attributes
            .clone()
            .map(|m| serde_json::to_value(m).unwrap_or_default());

        let new_user = db::UserRow {
            user_id: user_id_val,
            username: req.username.clone(),
            full_name: normalize_full_name(req.first_name.clone()),
            email: req.email.clone(),
            email_verified: req.email_verified.unwrap_or(false),
            phone_number: req
                .attributes
                .as_ref()
                .and_then(|a| a.get("phone_number").cloned()),
            disabled: !req.enabled.unwrap_or(true),
            attributes: attributes_json,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        diesel::insert_into(app_user)
            .values(&new_user)
            .get_result(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn get_user(&self, user_id_val: &str) -> RepoResult<Option<db::UserRow>> {
        debug!("Getting user: {}", user_id_val);
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        app_user
            .filter(user_id.eq(user_id_val))
            .first::<db::UserRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn update_user(
        &self,
        user_id_val: &str,
        req: &user_dto::UserUpsert,
    ) -> RepoResult<Option<db::UserRow>> {
        debug!("Updating user: {}", user_id_val);
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        let attributes_json = req
            .attributes
            .clone()
            .map(|m| serde_json::to_value(m).unwrap_or_default());

        diesel::update(app_user.filter(user_id.eq(user_id_val)))
            .set((
                username.eq(req.username.clone()),
                full_name.eq(normalize_full_name(req.first_name.clone())),
                email.eq(req.email.clone()),
                email_verified.eq(req.email_verified.unwrap_or(false)),
                disabled.eq(!req.enabled.unwrap_or(true)),
                attributes.eq(attributes_json),
                updated_at.eq(chrono::Utc::now()),
            ))
            .get_result::<db::UserRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn delete_user(&self, user_id_val: &str) -> RepoResult<u64> {
        debug!("Deleting user: {}", user_id_val);
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        diesel::delete(app_user.filter(user_id.eq(user_id_val)))
            .execute(&mut conn)
            .await
            .map(|n| n as u64)
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn search_users(&self, req: &user_dto::UserSearch) -> RepoResult<Vec<db::UserRow>> {
        debug!(
            "Searching users: search={:?}",
            req.search
        );
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;
        let mut query = app_user.into_boxed();

        let exact = req.exact.unwrap_or(false);
        let search_filter = normalize_search_field(req.search.as_ref());
        let username_filter = normalize_search_field(req.username.as_ref());
        let first_name_filter = normalize_search_field(req.first_name.as_ref());
        let last_name_filter = normalize_search_field(req.last_name.as_ref());
        let email_filter = normalize_search_field(req.email.as_ref());

        let attribute_filters: Vec<(String, String)> = req
            .attributes
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|(key, value)| {
                        let key = normalize_search_field(Some(key))?;
                        let value = normalize_search_field(Some(value))?;
                        Some((key, value))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let has_identity_filter = search_filter.is_some()
            || username_filter.is_some()
            || first_name_filter.is_some()
            || last_name_filter.is_some()
            || email_filter.is_some()
            || !attribute_filters.is_empty();

        // Defensive guard for KC federation lookups:
        // if the request contains no identity filters, do not return
        // arbitrary users.
        if !has_identity_filter {
            return Ok(vec![]);
        }

        if let Some(search_val) = search_filter {
            if exact {
                query = query.filter(
                    email
                        .eq(search_val.clone())
                        .or(username.eq(search_val.clone()))
                        .or(full_name.eq(search_val)),
                );
            } else {
                let pattern = format!("%{}%", search_val);
                query = query.filter(
                    email
                        .ilike(pattern.clone())
                        .or(username.ilike(pattern.clone()))
                        .or(full_name.ilike(pattern)),
                );
            }
        }

        if let Some(username_val) = username_filter {
            if exact {
                query = query.filter(username.eq(username_val));
            } else {
                query = query.filter(username.ilike(format!("%{}%", username_val)));
            }
        }

        if let Some(email_val) = email_filter {
            if exact {
                query = query.filter(email.eq(email_val));
            } else {
                query = query.filter(email.ilike(format!("%{}%", email_val)));
            }
        }

        if exact {
            match (first_name_filter.clone(), last_name_filter.clone()) {
                (Some(first_name_val), Some(last_name_val)) => {
                    query = query.filter(full_name.eq(format!("{first_name_val} {last_name_val}")));
                }
                (Some(first_name_val), None) => {
                    query = query.filter(full_name.eq(first_name_val));
                }
                (None, Some(last_name_val)) => {
                    query = query.filter(full_name.eq(last_name_val));
                }
                (None, None) => {}
            }
        } else {
            if let Some(first_name_val) = first_name_filter.clone() {
                query = query.filter(full_name.ilike(format!("%{}%", first_name_val)));
            }
            if let Some(last_name_val) = last_name_filter.clone() {
                query = query.filter(full_name.ilike(format!("%{}%", last_name_val)));
            }
        }

        for (key, value) in attribute_filters {
            let contains_value = serde_json::json!({ key: value });
            query = query.filter(attributes.contains(contains_value));
        }

        if let Some(enabled_val) = req.enabled {
            query = query.filter(disabled.eq(!enabled_val));
        }

        if let Some(email_verified_val) = req.email_verified {
            query = query.filter(email_verified.eq(email_verified_val));
        }

        let limit_val = req.max_results.unwrap_or(50).clamp(1, 200) as i64;
        let offset_val = req.first_result.unwrap_or(0).max(0) as i64;

        query
            .order(created_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .load::<db::UserRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn resolve_user_by_phone(
        &self,
        phone: &str,
    ) -> RepoResult<Option<db::UserRow>> {
        debug!("Resolving user by phone: {}", phone);
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        app_user
            .filter(username.eq(phone))
            .first::<db::UserRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn find_users_by_phone(&self, phone: &str) -> RepoResult<Vec<db::UserRow>> {
        debug!("Finding users by phone: {}", phone);
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        app_user
            .filter(phone_number.eq(phone).or(username.eq(phone)))
            .order(created_at.desc())
            .load::<db::UserRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn resolve_or_create_user_by_phone(
        &self,
        phone: &str,
    ) -> RepoResult<(db::UserRow, bool)> {
        debug!("Resolving or creating user by phone: {}", phone);
        if let Some(user) = self.resolve_user_by_phone(phone).await? {
            return Ok((user, false));
        }

        use backend_model::schema::app_user::dsl::*;
        let user_id_val = backend_core::id::user_id().map_err(|e| backend_core::Error::internal("ID_GEN", e))?;
        let mut conn = self.get_conn().await?;

        let attributes_json = serde_json::json!({ "phone_number": phone });

        let new_user = db::UserRow {
            user_id: user_id_val,
            username: phone.to_owned(),
            email: None,
            email_verified: false,
            phone_number: Some(phone.to_owned()),
            full_name: None,
            disabled: false,
            attributes: Some(attributes_json),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let user = diesel::insert_into(app_user)
            .values(&new_user)
            .get_result(&mut conn)
            .await?;

        Ok((user, true))
    }

    #[instrument(skip(self))]
    async fn upsert_user_data(&self, input: UserDataUpsertInput) -> RepoResult<db::UserDataRow> {
        debug!(
            "Upserting user data: user_id={}, name={}",
            input.user_id, input.name
        );
        use backend_model::schema::app_user_data::dsl::*;

        let mut conn = self.get_conn().await?;
        let now = chrono::Utc::now();

        diesel::insert_into(app_user_data)
            .values((
                user_id.eq(input.user_id),
                name.eq(input.name),
                data_type.eq(input.data_type),
                content.eq(input.content),
                eager_fetch.eq(input.eager_fetch),
                created_at.eq(now),
                updated_at.eq(now),
            ))
            .on_conflict((user_id, name, data_type))
            .do_update()
            .set((
                content.eq(excluded(content)),
                eager_fetch.eq(excluded(eager_fetch)),
                updated_at.eq(now),
            ))
            .get_result::<db::UserDataRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn list_user_data(
        &self,
        user_id_val: &str,
        eager_fetch_only: bool,
    ) -> RepoResult<Vec<db::UserDataRow>> {
        debug!(
            "Listing user data: user_id={}, eager_only={}",
            user_id_val, eager_fetch_only
        );
        use backend_model::schema::app_user_data::dsl::*;

        let mut conn = self.get_conn().await?;
        let mut query = app_user_data.filter(user_id.eq(user_id_val)).into_boxed();
        if eager_fetch_only {
            query = query.filter(eager_fetch.eq(true));
        }

        query
            .order((name.asc(), data_type.asc()))
            .load::<db::UserDataRow>(&mut conn)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip(self))]
    async fn update_phone_number(&self, user_id_val: &str, phone: &str) -> RepoResult<()> {
        debug!(
            "Updating user phone number: user_id={}, phone={}",
            user_id_val, phone
        );
        use backend_model::schema::app_user::dsl::*;

        let mut conn = self.get_conn().await?;

        if let Some(user) = app_user
            .filter(user_id.eq(user_id_val))
            .first::<db::UserRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::<backend_core::Error>::into)?
        {
            let mut attrs = user.attributes.unwrap_or_else(|| serde_json::json!({}));
            if !attrs.is_object() {
                attrs = serde_json::json!({});
            }
            if let Some(map) = attrs.as_object_mut() {
                map.insert(
                    "phone_number".to_owned(),
                    serde_json::Value::String(phone.to_owned()),
                );
            }

            diesel::update(app_user.filter(user_id.eq(user_id_val)))
                .set((
                    phone_number.eq(Some(phone.to_owned())),
                    attributes.eq(Some(attrs)),
                    updated_at.eq(chrono::Utc::now()),
                ))
                .execute(&mut conn)
                .await
                .map_err(Into::<backend_core::Error>::into)?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_full_name(&self, user_id_val: &str, name: &str) -> RepoResult<()> {
        debug!("Updating user full name: user_id={}", user_id_val);
        use backend_model::schema::app_user::dsl::*;

        let normalized = name.trim();
        if normalized.is_empty() {
            return Ok(());
        }

        let mut conn = self.get_conn().await?;

        diesel::update(app_user.filter(user_id.eq(user_id_val)))
            .set((
                full_name.eq(Some(normalized.to_owned())),
                updated_at.eq(chrono::Utc::now()),
            ))
            .execute(&mut conn)
            .await
            .map_err(Into::<backend_core::Error>::into)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_user_metadata(&self, user_id_val: &str) -> RepoResult<serde_json::Value> {
        debug!("Getting user metadata: user_id={}", user_id_val);
        use backend_model::schema::app_user_data::dsl::*;

        let mut conn = self.get_conn().await?;

        let rows = app_user_data
            .filter(user_id.eq(user_id_val))
            .filter(data_type.eq(USER_METADATA_DATA_TYPE))
            .select((name, content))
            .load::<(String, serde_json::Value)>(&mut conn)
            .await
            .map_err(Into::<backend_core::Error>::into)?;

        let mut out = serde_json::Map::new();
        for (key, value) in rows {
            out.insert(key, value);
        }

        Ok(serde_json::Value::Object(out))
    }

    #[instrument(skip(self))]
    async fn update_metadata(
        &self,
        user_id_val: &str,
        metadata_patch: serde_json::Value,
        eager_patch: Option<serde_json::Value>,
    ) -> RepoResult<()> {
        debug!("Updating user metadata: user_id={}", user_id_val);
        use backend_model::schema::app_user::dsl as user_dsl;
        use backend_model::schema::app_user_data::dsl::*;
        let mut conn = self.get_conn().await?;

        let Some(patch_obj) = metadata_patch.as_object() else {
            return Ok(());
        };
        let eager_obj = eager_patch.as_ref().and_then(serde_json::Value::as_object);

        let now = chrono::Utc::now();
        let updated_users =
            diesel::update(user_dsl::app_user.filter(user_dsl::user_id.eq(user_id_val)))
                .set(user_dsl::updated_at.eq(now))
                .execute(&mut conn)
                .await
                .map_err(Into::<backend_core::Error>::into)?;
        if updated_users == 0 {
            return Ok(());
        }

        for (key, patch_value) in patch_obj {
            let key_name = key.clone();
            if patch_value.is_null() {
                diesel::delete(
                    app_user_data
                        .filter(user_id.eq(user_id_val))
                        .filter(name.eq(&key_name))
                        .filter(data_type.eq(USER_METADATA_DATA_TYPE)),
                )
                .execute(&mut conn)
                .await
                .map_err(Into::<backend_core::Error>::into)?;
                continue;
            }

            let existing = app_user_data
                .filter(user_id.eq(user_id_val))
                .filter(name.eq(&key_name))
                .filter(data_type.eq(USER_METADATA_DATA_TYPE))
                .select((content, eager_fetch))
                .first::<(serde_json::Value, bool)>(&mut conn)
                .await
                .optional()
                .map_err(Into::<backend_core::Error>::into)?;
            let (mut merged_value, existing_eager) =
                existing.unwrap_or_else(|| (serde_json::json!({}), false));
            let resolved_eager = eager_obj
                .and_then(|map| map.get(&key_name))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(existing_eager);

            merge_json_value(&mut merged_value, patch_value);

            diesel::insert_into(app_user_data)
                .values((
                    user_id.eq(user_id_val),
                    name.eq(&key_name),
                    data_type.eq(USER_METADATA_DATA_TYPE),
                    content.eq(merged_value),
                    eager_fetch.eq(resolved_eager),
                    created_at.eq(now),
                    updated_at.eq(now),
                ))
                .on_conflict((user_id, name, data_type))
                .do_update()
                .set((
                    content.eq(excluded(content)),
                    eager_fetch.eq(resolved_eager),
                    updated_at.eq(now),
                ))
                .execute(&mut conn)
                .await
                .map_err(Into::<backend_core::Error>::into)?;
        }

        Ok(())
    }
}
