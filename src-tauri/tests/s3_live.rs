use amazon_s3_drive_lib::{
    models::{AwsCredentials, S3Error},
    s3_client::S3Client,
};
use std::{env, fs};

#[tokio::test]
#[ignore = "requires a dedicated AWS S3 test bucket and an authenticated AWS profile"]
async fn conditional_delete_preserves_replacement() {
    let bucket = env::var("S3_DRIVE_TEST_BUCKET")
        .ok()
        .filter(|bucket| !bucket.trim().is_empty())
        .expect("set S3_DRIVE_TEST_BUCKET to a dedicated test bucket");
    let region = env::var("S3_DRIVE_TEST_REGION")
        .ok()
        .filter(|region| !region.trim().is_empty())
        .expect("set S3_DRIVE_TEST_REGION to the bucket region");
    let client = S3Client::new(
        &AwsCredentials {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            session_token: None,
            region,
            use_iam_role: true,
        },
        bucket,
        None,
    )
    .await
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.txt");
    let replacement = dir.path().join("replacement.txt");
    fs::write(&original, b"original").unwrap();
    fs::write(&replacement, b"replacement").unwrap();
    let key = format!("s3-drive-integration/{:032x}.txt", rand::random::<u128>());

    let result = async {
        let approved_etag = client.upload_file(&key, &original).await?;
        let replacement_etag = client.upload_file(&key, &replacement).await?;
        if approved_etag == replacement_etag {
            return Err("the replacement must have a different ETag".into());
        }
        match client.delete_object_if_match(&key, &approved_etag).await {
            Err(S3Error::PreconditionFailed) => {}
            outcome => return Err(format!("expected stale ETag rejection, got {outcome:?}").into()),
        }
        let remaining = client.head_object(&key).await?;
        if remaining.etag != replacement_etag {
            return Err("the current object is no longer the replacement".into());
        }
        client
            .delete_object_if_match(&key, &replacement_etag)
            .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    if let Err(error) = result {
        let cleanup = client.delete_object(&key).await;
        panic!("conditional delete failed: {error}; cleanup: {cleanup:?}");
    }
}
