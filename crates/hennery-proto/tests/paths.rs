//! The path-segment rule (ACP core §7, kernel spec §5.2; plan 6c).

use hennery_proto::paths::is_within;

#[test]
fn a_root_holds_itself_and_what_lies_under_it_by_whole_segments() {
    for (path, root, within) in [
        ("/p/acme", "/p/acme", true),
        ("/p/acme/x", "/p/acme", true),
        ("/p/acme/x/y", "/p/acme", true),
        ("/p/acme-infra", "/p/acme", false),
        ("/p/acmex", "/p/acme", false),
        ("/p", "/p/acme", false),
        ("/home/u2", "/home/u", false),
        ("/home/u", "/home/u2", false),
        ("/anything", "/", true),
        ("/", "/", true),
        ("relative", "/", false),
    ] {
        assert_eq!(is_within(path, root), within, "{path} in {root}");
    }
}

#[test]
fn case_is_never_folded() {
    assert!(!is_within("/Users/U/src", "/Users/u"));
    assert!(!is_within("/users/u/src", "/Users/u"));
}
