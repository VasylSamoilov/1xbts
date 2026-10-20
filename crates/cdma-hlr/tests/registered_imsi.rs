use std::sync::Arc;

use cdma_hlr::model::{MobileIdentityKey, NumberPlan, NumberType};
use cdma_hlr::proto;
use cdma_hlr::proto::hlr_service_server::HlrService;
use cdma_hlr::repository::{HlrRepository, PostgresHlrRepository};
use cdma_hlr::service::HlrServiceImpl;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tonic::{Code, Request};
use uuid::Uuid;

const IMSI: &str = "310005551239876";
const SERVING_NODE: &str = "femto-fcs-1";
const ESN: u32 = 0x80b6_6bd7;
const MEID: &str = "a000002cc81a35";

async fn subscriber(
    repo: &PostgresHlrRepository,
    pool: &PgPool,
    number: &str,
    esn: u32,
    meid: &str,
    node: &str,
) -> Uuid {
    let subscriber = repo
        .upsert_subscriber(
            number,
            "",
            "active",
            NumberType::NetworkSpecific,
            NumberPlan::IsdnE164,
        )
        .await
        .unwrap();
    repo.upsert_identity(subscriber.subscriber_id, Some(IMSI), Some(esn), Some(meid))
        .await
        .unwrap();
    sqlx::query("INSERT INTO registration_bindings (subscriber_id, serving_bs_id, state, imsi, esn, meid) VALUES ($1,$2,'registered',$3,$4,$5)")
        .bind(subscriber.subscriber_id).bind(node).bind(IMSI).bind(i64::from(esn)).bind(meid).execute(pool).await.unwrap();
    subscriber.subscriber_id
}

async fn lookup(
    service: &HlrServiceImpl,
    imsi: &str,
    node: &str,
) -> Result<proto::ResolveSubscriberByIdentityResponse, tonic::Status> {
    service
        .resolve_subscriber_by_registered_imsi(Request::new(
            proto::ResolveSubscriberByRegisteredImsiRequest {
                imsi: imsi.to_string(),
                serving_bs_id: node.to_string(),
            },
        ))
        .await
        .map(tonic::Response::into_inner)
}

async fn verify(pool: PgPool) {
    let repo = Arc::new(PostgresHlrRepository::new(pool.clone()));
    repo.run_migrations().await.unwrap();
    let first = subscriber(&repo, &pool, "555100", ESN, MEID, SERVING_NODE).await;
    let service = HlrServiceImpl::new(repo.clone());
    let result = lookup(&service, IMSI, SERVING_NODE).await.unwrap();
    assert_eq!(result.subscriber.unwrap().phone_number, "555100");
    assert_eq!(result.binding.unwrap().esn, Some(ESN));
    let restarted = HlrServiceImpl::new(Arc::new(PostgresHlrRepository::new(pool.clone())));
    assert_eq!(
        lookup(&restarted, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .unwrap()
            .phone_number,
        "555100"
    );
    assert!(
        lookup(&service, IMSI, "other-node")
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
    assert!(
        lookup(&service, "310005551239999", SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
    assert_eq!(
        lookup(&service, "bad-imsi", SERVING_NODE)
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(
        lookup(&service, IMSI, "").await.unwrap_err().code(),
        Code::InvalidArgument
    );

    for state in ["stale", "registered", "paged", "page_response_received"] {
        sqlx::query("UPDATE registration_bindings SET state=$1 WHERE subscriber_id=$2")
            .bind(state)
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            lookup(&service, IMSI, SERVING_NODE)
                .await
                .unwrap()
                .subscriber
                .is_some(),
            state != "stale"
        );
    }
    sqlx::query("UPDATE registration_bindings SET esn=$1 WHERE subscriber_id=$2")
        .bind(i64::from(ESN) + 1)
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
    sqlx::query("UPDATE registration_bindings SET esn=NULL WHERE subscriber_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_some()
    );
    sqlx::query("UPDATE registration_bindings SET meid=NULL WHERE subscriber_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
    sqlx::query("UPDATE registration_bindings SET esn=$1,meid=$2 WHERE subscriber_id=$3")
        .bind(i64::from(ESN))
        .bind("a000002cc81a36")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
    sqlx::query("UPDATE registration_bindings SET meid=$1 WHERE subscriber_id=$2")
        .bind(MEID)
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DROP INDEX idx_identity_canonical_imsi")
        .execute(&pool)
        .await
        .unwrap();
    let second = subscriber(
        &repo,
        &pool,
        "555105",
        ESN + 1,
        "a000002cc81a36",
        SERVING_NODE,
    )
    .await;
    assert_eq!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    sqlx::query(
        "UPDATE registration_bindings SET serving_bs_id='other-node' WHERE subscriber_id=$1",
    )
    .bind(second)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .unwrap()
            .phone_number,
        "555100"
    );
    assert_eq!(
        lookup(&service, IMSI, "other-node")
            .await
            .unwrap()
            .subscriber
            .unwrap()
            .phone_number,
        "555105"
    );
    assert!(
        repo.resolve_by_identity(&MobileIdentityKey::ImsiEsn {
            imsi: IMSI.to_string(),
            esn: ESN + 2
        })
        .await
        .unwrap()
        .is_none()
    );
    sqlx::query("DELETE FROM subscriber_identities WHERE subscriber_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        lookup(&service, IMSI, SERVING_NODE)
            .await
            .unwrap()
            .subscriber
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires HLR_TEST_DSN with permission to create a temporary database"]
async fn registered_imsi_lookup_survives_restart_and_rejects_unsafe_matches() {
    let admin_dsn = std::env::var("HLR_TEST_DSN").expect("set HLR_TEST_DSN");
    let admin = PgPoolOptions::new().connect(&admin_dsn).await.unwrap();
    let database = format!("hlr_imsi_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let (base, _) = admin_dsn.rsplit_once('/').unwrap();
    let pool = PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path TO hlr, public")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&format!("{base}/{database}"))
        .await
        .unwrap();
    let result = tokio::spawn(verify(pool.clone())).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    result.unwrap();
}
