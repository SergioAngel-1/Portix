use crate::model::{BindScope, Service};
use nucleo_matcher::{pattern::Pattern, Config, Matcher, Utf32Str};

/// Fuzzy search over services.
pub struct Query {
    matcher: Matcher,
    pattern: Option<Pattern>,
    query_string: String,
    scope_filters: Vec<ScopeFilter>,
}

/// A `scope:` selector parsed out of the fuzzy search terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeFilter {
    Local,
    Network,
    All,
    Address,
}

impl ScopeFilter {
    fn parse(term: &str) -> Option<Self> {
        let (key, value) = term.split_once(':')?;
        if !key.eq_ignore_ascii_case("scope") {
            return None;
        }
        if value.eq_ignore_ascii_case("local") {
            Some(Self::Local)
        } else if value.eq_ignore_ascii_case("network") {
            Some(Self::Network)
        } else if value.eq_ignore_ascii_case("all") {
            Some(Self::All)
        } else if value.eq_ignore_ascii_case("address") {
            Some(Self::Address)
        } else {
            None
        }
    }

    fn matches(self, service: &Service) -> bool {
        match self {
            Self::Local => service.socket.is_loopback(),
            Self::Network => service.socket.is_network_bound(),
            Self::All => matches!(service.socket.bind_scope, BindScope::AllInterfaces),
            Self::Address => matches!(service.socket.bind_scope, BindScope::Specific(_)),
        }
    }
}

fn matches_scopes(filters: &[ScopeFilter], service: &Service) -> bool {
    filters.iter().all(|filter| filter.matches(service))
}

impl Default for Query {
    fn default() -> Self {
        Self::new()
    }
}

impl Query {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            pattern: None,
            query_string: String::new(),
            scope_filters: Vec::new(),
        }
    }

    /// Update the search query.
    pub fn set_query(&mut self, query: &str) {
        self.query_string = query.to_string();
        self.scope_filters.clear();
        let mut fuzzy_terms = Vec::new();
        for term in query.split_whitespace() {
            if let Some(scope) = ScopeFilter::parse(term) {
                self.scope_filters.push(scope);
            } else {
                fuzzy_terms.push(term);
            }
        }

        let fuzzy_query = fuzzy_terms.join(" ");
        if fuzzy_query.is_empty() {
            self.pattern = None;
        } else {
            self.pattern = Some(Pattern::new(
                &fuzzy_query,
                nucleo_matcher::pattern::CaseMatching::Smart,
                nucleo_matcher::pattern::Normalization::Smart,
                nucleo_matcher::pattern::AtomKind::Fuzzy,
            ));
        }
    }

    /// Get the current query string.
    pub fn query(&self) -> &str {
        &self.query_string
    }

    /// Filter and score services based on the query.
    pub fn filter<'a>(&mut self, services: &'a [Service]) -> Vec<(usize, &'a Service)> {
        let scope_filters = &self.scope_filters;
        let Some(pattern) = &self.pattern else {
            return services
                .iter()
                .enumerate()
                .filter(|(_, service)| matches_scopes(scope_filters, service))
                .collect();
        };

        let mut scored: Vec<(u32, usize, &Service)> = Vec::new();

        for (idx, service) in services.iter().enumerate() {
            if !matches_scopes(scope_filters, service) {
                continue;
            }

            let haystack = self.service_to_string(service);
            let mut buf = Vec::new();
            let score = pattern.score(Utf32Str::new(&haystack, &mut buf), &mut self.matcher);
            if let Some(s) = score {
                scored.push((s, idx, service));
            }
        }

        // Sort by fuzzy score (higher is better).
        scored.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
        scored
            .into_iter()
            .map(|(_, idx, service)| (idx, service))
            .collect()
    }

    /// Convert a service to a searchable string.
    fn service_to_string(&self, service: &Service) -> String {
        use std::fmt::Write;
        // `write!` into a String is infallible; `let _ =` is the idiomatic
        // spelling and avoids the (impossible) unwrap.
        let mut buf = String::new();
        let _ = write!(buf, "{}", service.socket.port);

        if let Some(ref project) = service.project {
            let _ = write!(buf, " {}", project.display_name());
        }
        if let Some(ref framework) = service.framework {
            let _ = write!(buf, " {}", framework.display_name);
        }
        if let Some(ref process) = service.process {
            let _ = write!(buf, " {}", process.command_display());
        }
        if let Some(ref process) = service.process {
            let _ = write!(buf, " {}", process.pid);
        }
        if let Some(ref container) = service.container {
            let _ = write!(buf, " {}", container.id);
            if let Some(ref name) = container.name {
                let _ = write!(buf, " {}", name);
            }
        }

        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BindScope, Protocol, ServiceId, Socket, TcpState};
    use std::net::IpAddr;

    fn service(port: u16, addr: IpAddr) -> Service {
        let socket = Socket {
            protocol: Protocol::Tcp,
            local_addr: addr,
            port,
            state: TcpState::Listen,
            bind_scope: BindScope::from_ip(addr),
            inode: port as u64,
            uid: 1000,
            dual_stack: false,
        };
        Service {
            id: ServiceId::from_socket(&socket),
            socket,
            process: None,
            project: None,
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    #[test]
    fn test_empty_query_returns_all() {
        let mut query = Query::new();
        let services = vec![];
        let results = query.filter(&services);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_scope_filters_select_expected_bindings() {
        let services = vec![
            service(3000, "127.0.0.1".parse().unwrap()),
            service(3001, "0.0.0.0".parse().unwrap()),
            service(3002, "192.168.1.5".parse().unwrap()),
        ];
        let mut query = Query::new();

        query.set_query("scope:local");
        assert_eq!(query.filter(&services)[0].1.socket.port, 3000);

        query.set_query("scope:network");
        assert_eq!(
            query
                .filter(&services)
                .iter()
                .map(|(_, service)| service.socket.port)
                .collect::<Vec<_>>(),
            vec![3001, 3002]
        );

        query.set_query("scope:all");
        assert_eq!(query.filter(&services)[0].1.socket.port, 3001);

        query.set_query("scope:address");
        assert_eq!(query.filter(&services)[0].1.socket.port, 3002);
    }

    #[test]
    fn test_scope_filter_combines_with_fuzzy_search() {
        let services = vec![
            service(3000, "127.0.0.1".parse().unwrap()),
            service(3002, "192.168.1.5".parse().unwrap()),
        ];
        let mut query = Query::new();
        query.set_query("scope:network 3002");

        let results = query.filter(&services);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.socket.port, 3002);
    }

    #[test]
    fn test_scope_filters_are_case_insensitive() {
        let services = vec![service(3000, "127.0.0.1".parse().unwrap())];
        let mut query = Query::new();
        query.set_query("Scope:LOCAL");

        assert_eq!(query.filter(&services)[0].1.socket.port, 3000);
    }
}
