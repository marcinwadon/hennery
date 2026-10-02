//! The enumeration cache (ACP core §7; plan 6c decision 10): by owner and
//! host, for one connection, for its lifetime.

use hennery_proto::frames::Project;
use hennery_sessions::projects::{Enumeration, ProjectsCache};
use std::time::Duration;

fn enumeration(path: &str) -> Enumeration {
    Enumeration {
        items: vec![Project { path: path.into() }],
        partial: false,
        home: None,
    }
}

#[test]
fn an_enumeration_is_served_only_to_its_owner_for_its_connection() {
    let cache = ProjectsCache::new(Duration::from_secs(60));
    cache.put("owner-a", "host-1", 7, enumeration("/a"));
    assert_eq!(cache.get("owner-a", "host-1", 7), Some(enumeration("/a")));
    assert_eq!(cache.get("owner-b", "host-1", 7), None, "another owner was served");
    assert_eq!(cache.get("owner-a", "host-2", 7), None);
    assert_eq!(cache.get("owner-a", "host-1", 8), None, "another connection was served");
    cache.put("owner-b", "host-1", 7, enumeration("/b"));
    cache.put("owner-a", "host-2", 9, enumeration("/c"));
    cache.forget("host-1");
    assert_eq!(cache.get("owner-a", "host-1", 7), None);
    assert_eq!(cache.get("owner-b", "host-1", 7), None);
    assert_eq!(cache.get("owner-a", "host-2", 9), Some(enumeration("/c")));
}

#[test]
fn an_enumeration_expires() {
    let cache = ProjectsCache::new(Duration::from_millis(50));
    cache.put("owner-a", "host-1", 7, enumeration("/a"));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(cache.get("owner-a", "host-1", 7), None);
}
