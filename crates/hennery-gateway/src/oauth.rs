#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    /// Gateway spec §4.1: RFC 8414 inserted before appended, then OpenID
    /// configuration.
    #[test]
    fn server_metadata_is_tried_inserted_then_appended_then_openid() {
        let found: Vec<String> = server_metadata_urls(&url("https://as.example/tenant/"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            found,
            [
                "https://as.example/.well-known/oauth-authorization-server/tenant",
                "https://as.example/tenant/.well-known/oauth-authorization-server",
                "https://as.example/tenant/.well-known/openid-configuration",
                "https://as.example/.well-known/openid-configuration/tenant",
            ]
        );
        let root: Vec<String> = server_metadata_urls(&url("https://as.example"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            root,
            [
                "https://as.example/.well-known/oauth-authorization-server",
                "https://as.example/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn the_resource_document_is_tried_path_inserted_then_at_the_origin() {
        let found: Vec<String> = resource_metadata_urls(&url("https://mcp.example/v1/mcp?x=1"))
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            found,
            [
                "https://mcp.example/.well-known/oauth-protected-resource/v1/mcp",
                "https://mcp.example/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn a_challenge_s_resource_metadata_is_read_quoted_or_not() {
        for (challenge, expected) in [
            (
                r#"Bearer realm="x", resource_metadata="https://m.example/.well-known/oauth-protected-resource""#,
                Some("https://m.example/.well-known/oauth-protected-resource"),
            ),
            (
                "Bearer resource_metadata=https://m.example/prm, error=\"invalid_token\"",
                Some("https://m.example/prm"),
            ),
            (
                r#"Bearer RESOURCE_METADATA="https://m.example/a\"b""#,
                Some("https://m.example/a\"b"),
            ),
            (r#"Bearer xresource_metadata="https://evil.example""#, None),
            ("Bearer realm=\"x\"", None),
        ] {
            assert_eq!(param_resource_metadata(challenge).as_deref(), expected, "{challenge}");
        }
    }

    #[test]
    fn the_s256_challenge_is_rfc_7636_s() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(verifier().len(), 43);
    }

    #[test]
    fn expiry_keeps_a_60_second_margin_and_unknown_stays_unknown() {
        assert_eq!(expires_at(1000, Some(3600)), Some(1000 + 3540));
        assert_eq!(expires_at(1000, Some(10)), Some(1000));
        assert_eq!(expires_at(1000, None), None);
    }

    #[test]
    fn a_secret_client_uses_basic_unless_only_post_is_offered() {
        assert_eq!(auth_method_for(false, &[]), AuthMethod::None);
        assert_eq!(auth_method_for(true, &[]), AuthMethod::Basic);
        assert_eq!(auth_method_for(true, &["client_secret_post".into()]), AuthMethod::Post);
        assert_eq!(
            auth_method_for(true, &["client_secret_post".into(), "client_secret_basic".into()]),
            AuthMethod::Basic
        );
    }

    /// O13: a token endpoint the policy refuses, or one at plain `http` to
    /// a name, is refused before anything is sent; neither is a vendor's
    /// refusal.
    #[tokio::test]
    async fn a_refused_token_endpoint_is_egress_refused() {
        let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap();
        let public = egress.client(hennery_kernel::egress::Allowance::PublicOnly);
        let internal = egress.client(hennery_kernel::egress::Allowance::InternalNetwork);
        let grant = Grant::Refresh {
            refresh_token: "r",
            scopes: &[],
        };
        for (client, endpoint) in [
            (&public, "https://127.0.0.1:1/token"),
            (&internal, "http://10.0.0.1/token"),
            (&internal, "http://as.example/token"),
        ] {
            let token_client = TokenClient {
                client_id: "c".into(),
                secret: None,
                auth_method: AuthMethod::None,
                token_endpoint: endpoint.into(),
            };
            let err = token(client, &token_client, &grant, None).await.unwrap_err();
            assert!(matches!(err, TokenError::EgressRefused(_)), "{endpoint}: {err:?}");
            assert!(!err.message().contains("/token"), "origins only: {}", err.message());
        }
    }

    /// O13: a registration endpoint that is not `https` (or loopback
    /// `http`) is refused before anything is sent, whatever discovery let
    /// through.
    #[tokio::test]
    async fn a_plain_http_registration_endpoint_is_egress_refused() {
        let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap();
        let internal = egress.client(hennery_kernel::egress::Allowance::InternalNetwork);
        let err = register(
            &internal,
            &url("http://as.example/register"),
            "https://h.example/cb",
            &[],
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, RegisterError::EgressRefused(_)), "{err:?}");
    }

    #[test]
    fn issuers_compare_but_for_one_final_slash() {
        assert!(same_issuer("https://as.example/", "https://as.example"));
        assert!(same_issuer("https://as.example/t", "https://as.example/t/"));
        assert!(!same_issuer("https://as.example/t", "https://as.example/u"));
        assert!(!same_issuer("", "https://as.example"));
    }
}
