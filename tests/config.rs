use std::path::PathBuf;

use modelvault::config::{ModelVaultConfig, RemoteDefinition};
use tempfile::tempdir;

#[test]
fn named_remote_round_trips_and_resolves_default() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    let remote_path = temp.path().join("remote");

    let mut config = ModelVaultConfig::default();
    config.add_filesystem_remote("origin", remote_path.clone()).unwrap();
    config.set_default("origin").unwrap();
    config.save(&store).unwrap();

    let loaded = ModelVaultConfig::load(&store).unwrap();
    let (name, remote) = loaded.resolve(None).unwrap();
    assert_eq!(name, "origin");
    assert_eq!(remote.kind, "filesystem");
    assert_eq!(remote.path.as_deref(), Some(remote_path.as_path()));
}

#[test]
fn s3_remote_round_trips_with_minio_settings() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");

    let remote = RemoteDefinition::s3(
        "models".to_string(),
        Some("team/modelvault/".to_string()),
        Some("us-east-1".to_string()),
        Some("http://127.0.0.1:9000".to_string()),
        true,
        None,
        5,
    )
    .unwrap();

    let mut config = ModelVaultConfig::default();
    config.add_s3_remote("minio", remote).unwrap();
    config.set_default("minio").unwrap();
    config.save(&store).unwrap();

    let loaded = ModelVaultConfig::load(&store).unwrap();
    let (_, remote) = loaded.resolve(None).unwrap();
    assert_eq!(remote.kind, "s3");
    assert_eq!(remote.bucket.as_deref(), Some("models"));
    assert_eq!(remote.prefix.as_deref(), Some("team/modelvault"));
    assert_eq!(remote.region.as_deref(), Some("us-east-1"));
    assert_eq!(remote.endpoint.as_deref(), Some("http://127.0.0.1:9000"));
    assert!(remote.force_path_style);
    assert_eq!(remote.max_attempts, 5);
}

#[test]
fn remote_names_are_validated() {
    let mut config = ModelVaultConfig::default();
    assert!(config.add_filesystem_remote("bad name", PathBuf::from("x")).is_err());
    assert!(config.add_filesystem_remote("team-nas", PathBuf::from("x")).is_ok());
}

#[cfg(not(feature = "s3"))]
#[test]
fn s3_remote_requires_feature_when_opened() {
    use modelvault::object_store::open_remote_store;

    let remote = RemoteDefinition::s3(
        "models".to_string(),
        None,
        Some("us-east-1".to_string()),
        None,
        false,
        None,
        4,
    )
    .unwrap();

    let error = open_remote_store(&remote).unwrap_err();
    assert!(error
        .to_string()
        .contains("S3 support is not compiled in"));
}
