//! Native forks cannot inherit the environment's thread-bound worker grants.
use pockymoe_protocol::AgentProviderCapabilitiesDto;

pub(super) fn restrict(caps: &mut AgentProviderCapabilitiesDto, isolated: bool) {
    if isolated {
        caps.branching.fork = false;
        caps.branching.fork_at = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_native_forks_are_hidden_without_changing_host_or_live_protocol_features() {
        let mut host = AgentProviderCapabilitiesDto::conversational();
        host.branching.fork = true;
        host.branching.fork_at = true;
        let original = serde_json::to_value(&host).unwrap();
        restrict(&mut host, false);
        assert_eq!(serde_json::to_value(&host).unwrap(), original);

        let mut isolated = host;
        restrict(&mut isolated, true);
        assert!(!isolated.branching.fork && !isolated.branching.fork_at);
        assert!(isolated.sessions.resume && isolated.turns.start);
        assert!(isolated.usage.token_usage && isolated.usage.context_window);
        assert!(!isolated.sessions.import_local);
    }
}
