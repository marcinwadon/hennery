#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(connection_id: &str) -> Snapshot {
        Snapshot {
            connection_id: connection_id.into(),
            url: "https://mcp.example/mcp".into(),
            cred_kind: CredKind::OauthDcr,
            internal_network: false,
            source: ClientSource::Registered,
            client: registered_client("c".into(), None, AuthMethod::None, "https://as.example/token".into()),
            issuer: "https://as.example".into(),
            authorization_endpoint: "https://as.example/authorize".into(),
            iss_parameter: false,
            redirect_uri: "https://h.example/api/mcp/oauth/callback".into(),
            scopes: Vec::new(),
            resource: "https://mcp.example/mcp".into(),
            resource_param: true,
            verifier: Zeroizing::new("v".into()),
            registered_at: 0,
            auth_session: "s".into(),
        }
    }

    #[test]
    fn a_flow_is_single_use_and_needs_its_cookie() {
        let flows = Flows::default();
        let started = flows.start(snapshot("conn-a"), 100).unwrap();
        assert!(matches!(
            flows.take(&started.state, &["wrong", started.cookie.as_str()], 101),
            Taken::Taken(_)
        ));
        assert!(matches!(
            flows.take(&started.state, &[started.cookie.as_str()], 101),
            Taken::Unknown
        ));
        let other = flows.start(snapshot("conn-a"), 100).unwrap();
        // A cookie of the right length, not the flow's.
        let wrong = "0".repeat(other.cookie.len());
        assert!(matches!(
            flows.take(&other.state, &[wrong.as_str()], 101),
            Taken::Mismatch(_)
        ));
        // Consumed by the mismatch: the right cookie is too late.
        assert!(matches!(
            flows.take(&other.state, &[other.cookie.as_str()], 101),
            Taken::Unknown
        ));
    }

    #[test]
    fn a_flow_expires_and_a_new_one_supersedes_it() {
        let flows = Flows::default();
        let first = flows.start(snapshot("conn-a"), 100).unwrap();
        assert_eq!(first.expires_at, 100 + FLOW_TTL);
        assert!(matches!(
            flows.take(&first.state, &[first.cookie.as_str()], 100 + FLOW_TTL),
            Taken::Unknown
        ));
        let first = flows.start(snapshot("conn-a"), 100).unwrap();
        let second = flows.start(snapshot("conn-a"), 100).unwrap();
        assert!(matches!(
            flows.take(&first.state, &[first.cookie.as_str()], 101),
            Taken::Unknown
        ));
        assert!(matches!(
            flows.take(&second.state, &[second.cookie.as_str()], 100 + FLOW_TTL - 1),
            Taken::Taken(_)
        ));
    }

    #[test]
    fn at_most_sixteen_flows_live_and_an_edit_drops_its_connection_s() {
        let flows = Flows::default();
        let started: Vec<Started> = (0..MAX_FLOWS)
            .map(|i| flows.start(snapshot(&format!("conn-{i}")), 100).unwrap())
            .collect();
        assert!(flows.start(snapshot("conn-x"), 100).is_none());
        assert_eq!(flows.live("conn-0", 100), MAX_FLOWS - 1);
        flows.drop_connection("conn-0");
        assert!(matches!(
            flows.take(&started[0].state, &[started[0].cookie.as_str()], 100),
            Taken::Unknown
        ));
        assert!(flows.start(snapshot("conn-x"), 100).is_some());
    }

    /// api-8e-8f S3: a held registration is reused only for the same token
    /// endpoint and redirect URI.
    #[test]
    fn a_registration_is_reused_only_for_the_same_endpoint_and_redirect() {
        let flows = Flows::default();
        let client = registered_client("c".into(), None, AuthMethod::None, "https://as.example/token".into());
        flows.hold_registration(
            "conn-a",
            Registration {
                client,
                redirect_uri: "https://h.example/cb".into(),
                registered_at: 1,
            },
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h.example/cb")
                .is_some()
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/other", "https://h.example/cb")
                .is_none()
        );
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h2.example/cb")
                .is_none()
        );
        assert!(
            flows
                .registration("conn-b", "https://as.example/token", "https://h.example/cb")
                .is_none()
        );
        flows.drop_connection("conn-a");
        assert!(
            flows
                .registration("conn-a", "https://as.example/token", "https://h.example/cb")
                .is_none()
        );
    }

    /// An edit forgets that the server refused `resource` (the review's
    /// R1): the next Connect sends it again.
    #[test]
    fn dropping_a_connection_forgets_its_refused_resource() {
        let flows = Flows::default();
        flows.refuse_resource("conn-a", "https://mcp.example/mcp");
        flows.refuse_resource("conn-b", "https://mcp.example/mcp");
        flows.drop_connection("conn-a");
        assert!(!flows.resource_refused("conn-a", "https://mcp.example/mcp"));
        assert!(flows.resource_refused("conn-b", "https://mcp.example/mcp"));
    }

    /// An expired flow does not count toward `MAX_FLOWS`.
    #[test]
    fn expired_flows_are_not_live() {
        let flows = Flows::default();
        for i in 0..3 {
            flows.start(snapshot(&format!("conn-{i}")), 100).unwrap();
        }
        assert_eq!(flows.live("none", 100 + FLOW_TTL - 1), 3);
        assert_eq!(flows.live("none", 100 + FLOW_TTL), 0);
    }

    #[test]
    fn a_cookie_s_name_comes_from_its_state() {
        let name = cookie_name("abc");
        assert_eq!(name.len(), FLOW_COOKIE_PREFIX.len() + 16);
        assert_ne!(name, cookie_name("abd"));
    }
}
