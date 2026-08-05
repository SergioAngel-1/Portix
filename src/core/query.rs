use crate::model::Service;
use nucleo_matcher::{pattern::Pattern, Config, Matcher, Utf32Str};

/// Fuzzy search over services.
pub struct Query {
    matcher: Matcher,
    pattern: Option<Pattern>,
    query_string: String,
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
        }
    }

    /// Update the search query.
    pub fn set_query(&mut self, query: &str) {
        self.query_string = query.to_string();
        if query.is_empty() {
            self.pattern = None;
        } else {
            self.pattern = Some(Pattern::new(
                query,
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
        match &self.pattern {
            None => services.iter().enumerate().collect(),
            Some(pattern) => {
                let mut scored: Vec<(u32, usize, &Service)> = Vec::new();

                for (idx, service) in services.iter().enumerate() {
                    let haystack = self.service_to_string(service);
                    let mut buf = Vec::new();
                    let score =
                        pattern.score(Utf32Str::new(&haystack, &mut buf), &mut self.matcher);
                    if let Some(s) = score {
                        scored.push((s, idx, service));
                    }
                }

                // Sort by score (higher is better)
                scored.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
                scored
                    .into_iter()
                    .map(|(_, idx, service)| (idx, service))
                    .collect()
            }
        }
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

    #[test]
    fn test_empty_query_returns_all() {
        let mut query = Query::new();
        let services = vec![];
        let results = query.filter(&services);
        assert_eq!(results.len(), 0);
    }
}
