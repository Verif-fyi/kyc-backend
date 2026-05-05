use anyhow::Result;
use backend_migrate::connect_postgres_and_migrate;
use backend_model::user::{UserAttributes, UserSearch, UserUpsert};
use backend_model::schema::app_user;
use backend_repository::{UserRepo, UserRepository};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

fn make_attributes(device_id: &str) -> UserAttributes {
    UserAttributes::from([(String::from("device_id"), device_id.to_owned())])
}

fn make_search_request(attributes: Option<UserAttributes>) -> UserSearch {
    UserSearch {
        search: None,
        username: None,
        first_name: None,
        last_name: None,
        email: None,
        enabled: None,
        email_verified: None,
        exact: Some(true),
        attributes,
        first_result: None,
        max_results: None,
    }
}

#[tokio::test]
async fn search_users_filters_by_attributes_and_rejects_realm_only_queries() -> Result<()> {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(value) => value,
        Err(_) => {
            eprintln!("Skipping backend-repository user test because DATABASE_URL is not set");
            return Ok(());
        }
    };

    let pool = connect_postgres_and_migrate(&database_url).await?;
    let repo = UserRepository::new(pool.clone());

    let now = chrono::Utc::now().timestamp_micros();
    let realm = format!("user-repo-test-{now}");
    let device_a = format!("dvc_a_{now}");
    let device_b = format!("dvc_b_{now}");

    let user_a = repo
        .create_user(&UserUpsert {
            username: format!("user-a-{now}"),
            first_name: Some("Alice".to_owned()),
            last_name: Some("A".to_owned()),
            email: Some(format!("alice-{now}@example.com")),
            enabled: Some(true),
            email_verified: Some(false),
            attributes: Some(make_attributes(&device_a)),
        })
        .await?;

    let user_b = repo
        .create_user(&UserUpsert {
            username: format!("user-b-{now}"),
            first_name: Some("Bob".to_owned()),
            last_name: Some("B".to_owned()),
            email: Some(format!("bob-{now}@example.com")),
            enabled: Some(true),
            email_verified: Some(false),
            attributes: Some(make_attributes(&device_b)),
        })
        .await?;

    let by_device = repo
        .search_users(&make_search_request(
            Some(make_attributes(&device_b)),
        ))
        .await?;
    assert_eq!(by_device.len(), 1);
    assert_eq!(by_device[0].user_id, user_b.user_id);

    let not_found = repo
        .search_users(&make_search_request(
            Some(make_attributes("dvc_missing")),
        ))
        .await?;
    assert!(not_found.is_empty());

    let realm_only = repo
        .search_users(&make_search_request(None))
        .await?;
    assert!(realm_only.is_empty());

    {
        let mut conn = pool.get().await?;
        diesel::delete(app_user::table.filter(app_user::username.like(format!("%-{now}"))))
            .execute(&mut conn)
            .await?;
    }

    assert_ne!(user_a.user_id, user_b.user_id);
    Ok(())
}
