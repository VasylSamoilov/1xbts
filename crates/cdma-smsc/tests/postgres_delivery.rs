use cdma_smsc::model::{DeliveryAttemptState, SmsDestination, SmsState};
use cdma_smsc::repository::{
    CreateSubmissionOptions, DeliveryCompletion, PostgresSmscRepository, SmscRepository,
};
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[ignore = "requires SMSC_TEST_DSN with permission to create a temporary database"]
async fn claims_and_outcomes_are_atomic_across_database_connections() {
    let admin_dsn = std::env::var("SMSC_TEST_DSN").expect("set SMSC_TEST_DSN");
    let admin = PgPoolOptions::new().connect(&admin_dsn).await.unwrap();
    let database = format!("smsc_delivery_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let (base, _) = admin_dsn.rsplit_once('/').expect("a database DSN");
    let dsn = format!("{base}/{database}");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path TO smsc, public")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&dsn)
        .await
        .unwrap();
    let repository = PostgresSmscRepository::new(pool.clone());
    repository.run_migrations().await.unwrap();
    let submission = repository
        .create_submission(
            "1000",
            SmsDestination::PhoneNumber("1001".into()),
            "hello",
            None,
            None,
            CreateSubmissionOptions::default(),
        )
        .await
        .unwrap();
    let (first, duplicate) = tokio::join!(
        repository.claim_delivery(submission.sms_id, None),
        repository.claim_delivery(submission.sms_id, None),
    );
    let first = first.unwrap();
    let duplicate = duplicate.unwrap();
    assert_ne!(first.is_some(), duplicate.is_some());
    let attempt = first.or(duplicate).unwrap();
    let attempts = repository
        .get_delivery_attempts(submission.sms_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].state, DeliveryAttemptState::Paging);
    assert!(
        repository
            .finish_delivery(
                submission.sms_id,
                attempt.sms_delivery_attempt_id,
                DeliveryCompletion::Retry(Some("offline".into()))
            )
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .get_submission(submission.sms_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SmsState::Accepted
    );
    assert_eq!(
        repository
            .get_delivery_attempts(submission.sms_id)
            .await
            .unwrap()[0]
            .state,
        DeliveryAttemptState::Failed
    );
    let retry = repository
        .claim_delivery(submission.sms_id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.attempt_number, 2);
    assert!(
        !repository
            .finish_delivery(
                submission.sms_id,
                attempt.sms_delivery_attempt_id,
                DeliveryCompletion::Delivered
            )
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .get_submission(submission.sms_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SmsState::Paging
    );
    assert!(
        repository
            .finish_delivery(
                submission.sms_id,
                retry.sms_delivery_attempt_id,
                DeliveryCompletion::Delivered
            )
            .await
            .unwrap()
    );
    assert!(
        !repository
            .finish_delivery(
                submission.sms_id,
                retry.sms_delivery_attempt_id,
                DeliveryCompletion::Retry(None)
            )
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .get_submission(submission.sms_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SmsState::Delivered
    );
    let legacy = repository
        .create_submission(
            "1000",
            SmsDestination::PhoneNumber("1001".into()),
            "legacy retry",
            None,
            None,
            CreateSubmissionOptions::default(),
        )
        .await
        .unwrap();
    repository
        .claim_delivery(legacy.sms_id, None)
        .await
        .unwrap()
        .unwrap();
    repository
        .update_submission_state(legacy.sms_id, SmsState::Accepted, None)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/007_complete_retryable_attempts.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repository
            .get_submission(legacy.sms_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SmsState::Accepted
    );
    assert_eq!(
        repository
            .get_delivery_attempts(legacy.sms_id)
            .await
            .unwrap()[0]
            .state,
        DeliveryAttemptState::Failed
    );
    const SUBMIT_MESSAGE_TYPE: u8 = 0x02;
    let fingerprint = cdma_smsc::model::MoSmsFingerprint {
        teleservice_id: cdma_common::sms::TELESERVICE_WMT,
        message_type: SUBMIT_MESSAGE_TYPE,
        message_id: 7,
    };
    let (first_mo, repeated_mo) = tokio::join!(
        repository.create_or_get_recent_mo_submission(
            "1000",
            "1001",
            "retransmission",
            None,
            None,
            &fingerprint
        ),
        repository.create_or_get_recent_mo_submission(
            "1000",
            "1001",
            "retransmission",
            None,
            None,
            &fingerprint
        ),
    );
    let (first_mo, created) = first_mo.unwrap();
    let (repeated_mo, repeated_created) = repeated_mo.unwrap();
    assert_ne!(created, repeated_created);
    assert_eq!(first_mo.sms_id, repeated_mo.sms_id);
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
