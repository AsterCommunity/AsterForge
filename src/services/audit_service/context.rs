const MAX_AUDIT_IP_ADDRESS_LEN: usize = 45;
const MAX_AUDIT_USER_AGENT_LEN: usize = 512;

#[derive(Clone)]
pub struct AuditContext {
    pub user_id: i64,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Clone)]
pub struct AuditRequestInfo {
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

impl AuditContext {
    pub fn system() -> Self {
        Self {
            user_id: 0,
            ip_address: None,
            user_agent: None,
        }
    }

    pub fn from_request(req: &actix_web::HttpRequest, user_id: i64) -> Self {
        AuditRequestInfo::from_request(req).to_context(user_id)
    }
}

impl AuditRequestInfo {
    pub fn from_request(req: &actix_web::HttpRequest) -> Self {
        Self {
            ip_address: req
                .connection_info()
                .realip_remote_addr()
                .map(|value| bounded_audit_value(value, MAX_AUDIT_IP_ADDRESS_LEN)),
            user_agent: req
                .headers()
                .get(actix_web::http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(|value| bounded_audit_value(value, MAX_AUDIT_USER_AGENT_LEN)),
        }
    }

    pub fn to_context(&self, user_id: i64) -> AuditContext {
        AuditContext {
            user_id,
            ip_address: self.ip_address.clone(),
            user_agent: self.user_agent.clone(),
        }
    }
}

fn bounded_audit_value(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }

    let mut end = max_len;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AuditContext, AuditRequestInfo, MAX_AUDIT_IP_ADDRESS_LEN, MAX_AUDIT_USER_AGENT_LEN,
        bounded_audit_value,
    };
    use actix_web::test as actix_test;

    #[test]
    fn system_context_has_reserved_system_user_without_request_metadata() {
        let context = AuditContext::system();
        assert_eq!(context.user_id, 0);
        assert_eq!(context.ip_address, None);
        assert_eq!(context.user_agent, None);
    }

    #[test]
    fn bounded_audit_value_truncates_without_splitting_utf8() {
        assert_eq!(bounded_audit_value("abcdef", 3), "abc");
        assert_eq!(bounded_audit_value("abcdef", 0), "");
        assert_eq!(bounded_audit_value("猫猫猫", 4), "猫");
        assert_eq!(bounded_audit_value("猫猫猫", "猫猫猫".len()), "猫猫猫");
    }

    #[test]
    fn request_info_truncates_user_controlled_ip_and_user_agent() {
        let long_ip = "1".repeat(MAX_AUDIT_IP_ADDRESS_LEN + 32);
        let long_user_agent = "a".repeat(MAX_AUDIT_USER_AGENT_LEN + 32);
        let req = actix_test::TestRequest::default()
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", long_ip.as_str()))
            .insert_header(("User-Agent", long_user_agent.as_str()))
            .to_http_request();

        let info = AuditRequestInfo::from_request(&req);

        assert_eq!(
            info.ip_address.as_deref(),
            Some(&long_ip[..MAX_AUDIT_IP_ADDRESS_LEN])
        );
        assert_eq!(
            info.user_agent.as_deref(),
            Some(&long_user_agent[..MAX_AUDIT_USER_AGENT_LEN])
        );
    }

    #[test]
    fn request_info_uses_peer_when_forwarded_header_is_absent() {
        let req = actix_test::TestRequest::default()
            .peer_addr("198.51.100.7:12345".parse().unwrap())
            .insert_header(("User-Agent", "forge-test-agent"))
            .to_http_request();

        let info = AuditRequestInfo::from_request(&req);

        assert_eq!(info.ip_address.as_deref(), Some("198.51.100.7"));
        assert_eq!(info.user_agent.as_deref(), Some("forge-test-agent"));
    }

    #[test]
    fn request_info_to_context_preserves_metadata_for_user() {
        let info = AuditRequestInfo {
            ip_address: Some("203.0.113.8".to_string()),
            user_agent: Some("forge-test-agent".to_string()),
        };

        let context = info.to_context(42);

        assert_eq!(context.user_id, 42);
        assert_eq!(context.ip_address.as_deref(), Some("203.0.113.8"));
        assert_eq!(context.user_agent.as_deref(), Some("forge-test-agent"));
    }

    #[test]
    fn context_from_request_combines_request_metadata_and_user_id() {
        let req = actix_test::TestRequest::default()
            .peer_addr("203.0.113.9:12345".parse().unwrap())
            .insert_header(("User-Agent", "forge-context-test"))
            .to_http_request();

        let context = AuditContext::from_request(&req, 7);

        assert_eq!(context.user_id, 7);
        assert_eq!(context.ip_address.as_deref(), Some("203.0.113.9"));
        assert_eq!(context.user_agent.as_deref(), Some("forge-context-test"));
    }
}
