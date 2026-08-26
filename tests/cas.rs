use modelvault::cas::LocalCas;

#[test]
fn identical_content_is_stored_once() {
    let temp = tempfile::tempdir().unwrap();
    let cas = LocalCas::open(temp.path()).unwrap();

    let first = cas.put_bytes(b"same content").unwrap();
    let second = cas.put_bytes(b"same content").unwrap();

    assert!(first.was_new);
    assert!(!second.was_new);
    assert_eq!(first.id, second.id);
    assert_eq!(cas.read(&first.id).unwrap(), b"same content");
    assert!(cas.verify(&first.id).unwrap());
}
