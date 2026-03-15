//! Structured audit logging for sensitive operator operations.
//!
//! All audit events use the `audit` tracing target, allowing operators to
//! filter and route them to a separate log file or SIEM system via the
//! tracing subscriber configuration (e.g., `RUST_LOG=audit=info`).
//!
//! Each event includes structured fields: `event` (operation name),
//! contextual identifiers (addresses, hashes), and optional `block_number`
//! and `tx_hash` fields for chain-related operations.

/// Log an audit event for enclave registration.
pub fn log_enclave_registered(
    enclave_address: &str,
    image_hash: &str,
    tx_hash: &str,
    operator_address: &str,
) {
    tracing::info!(
        target: "audit",
        event = "enclave_registered",
        enclave_address = %enclave_address,
        image_hash = %image_hash,
        tx_hash = %tx_hash,
        operator = %operator_address,
        "Enclave registered on-chain"
    );
}

/// Log an audit event for enclave registration attempt (before tx submission).
pub fn log_enclave_registration_attempted(
    enclave_address: &str,
    pcr0: &str,
    skip_verify: bool,
) {
    tracing::info!(
        target: "audit",
        event = "enclave_registration_attempted",
        enclave_address = %enclave_address,
        pcr0 = %pcr0,
        skip_verify = skip_verify,
        "Enclave registration attempted"
    );
}

/// Log an audit event for a dispute submission (challenge detected and proof submitted).
pub fn log_dispute_submitted(
    result_id: &str,
    challenger: &str,
    tx_hash: &str,
) {
    tracing::info!(
        target: "audit",
        event = "dispute_submitted",
        result_id = %result_id,
        challenger = %challenger,
        tx_hash = %tx_hash,
        "Dispute proof submitted on-chain"
    );
}

/// Log an audit event for a failed dispute resolution.
pub fn log_dispute_failed(
    result_id: &str,
    challenger: &str,
    error: &str,
) {
    tracing::warn!(
        target: "audit",
        event = "dispute_failed",
        result_id = %result_id,
        challenger = %challenger,
        error = %error,
        "Dispute resolution failed"
    );
}

/// Log an audit event for a challenge detection.
pub fn log_challenge_detected(
    result_id: &str,
    challenger: &str,
    block_number: u64,
) {
    tracing::warn!(
        target: "audit",
        event = "challenge_detected",
        result_id = %result_id,
        challenger = %challenger,
        block_number = block_number,
        "Challenge detected on-chain"
    );
}

/// Log an audit event for result submission.
pub fn log_result_submitted(
    tx_hash: &str,
    model_name: &str,
    feature_count: usize,
) {
    tracing::info!(
        target: "audit",
        event = "result_submitted",
        tx_hash = %tx_hash,
        model_name = %model_name,
        feature_count = feature_count,
        "Inference result submitted on-chain"
    );
}

/// Log an audit event for prover slashing (dispute lost by prover).
pub fn log_prover_slashed(
    result_id: &str,
    prover_won: bool,
) {
    if !prover_won {
        tracing::warn!(
            target: "audit",
            event = "prover_slashed",
            result_id = %result_id,
            "Prover lost dispute -- stake slashed"
        );
    }
}

/// Log an audit event for configuration loaded at startup.
pub fn log_config_loaded(
    rpc_url: &str,
    contract_address: &str,
    dry_run: bool,
    nitro_verification: bool,
) {
    tracing::info!(
        target: "audit",
        event = "config_loaded",
        rpc_url = %rpc_url,
        contract_address = %contract_address,
        dry_run = dry_run,
        nitro_verification = nitro_verification,
        "Operator configuration loaded"
    );
}

/// Log an audit event for auto-finalization of a result.
pub fn log_result_finalized(
    result_id: &str,
    tx_hash: &str,
) {
    tracing::info!(
        target: "audit",
        event = "result_finalized",
        result_id = %result_id,
        tx_hash = %tx_hash,
        "Result auto-finalized on-chain"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that audit log functions do not panic when called without a
    /// global subscriber (the tracing macros gracefully no-op in that case).
    #[test]
    fn test_audit_log_functions_do_not_panic() {
        log_enclave_registered(
            "0x1234567890abcdef1234567890abcdef12345678",
            "0xabcdef",
            "0xdeadbeef",
            "0xoperator",
        );
        log_enclave_registration_attempted(
            "0x1234567890abcdef1234567890abcdef12345678",
            "0xpcr0hash",
            false,
        );
        log_dispute_submitted("0xresult1", "0xchallenger1", "0xtx1");
        log_dispute_failed("0xresult2", "0xchallenger2", "proof timeout");
        log_challenge_detected("0xresult3", "0xchallenger3", 12345);
        log_result_submitted("0xtx2", "iris-model", 4);
        log_prover_slashed("0xresult4", false);
        log_prover_slashed("0xresult5", true); // should not log (prover won)
        log_config_loaded(
            "http://localhost:8545",
            "0xcontract",
            false,
            true,
        );
        log_result_finalized("0xresult6", "0xtx3");
    }

    /// Verify that audit events use the correct target for filtering.
    /// This test uses a tracing subscriber with a filter to capture only
    /// `audit` target events and verify they are emitted.
    #[test]
    fn test_audit_events_use_audit_target() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        // Custom layer that captures events with target "audit"
        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_clone = captured.clone();

        let layer = TestAuditLayer { captured: captured_clone };

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        log_enclave_registered("0xaddr", "0xhash", "0xtx", "0xop");
        log_challenge_detected("0xresult", "0xchallenger", 100);
        log_result_submitted("0xtx", "model", 4);
        log_dispute_submitted("0xresult", "0xchallenger", "0xtx");
        log_dispute_failed("0xresult", "0xchallenger", "error");
        log_config_loaded("http://rpc", "0xcontract", false, true);
        log_result_finalized("0xresult", "0xtx");
        log_prover_slashed("0xresult", false);

        let events = captured.lock().unwrap();
        assert!(
            events.len() >= 8,
            "Expected at least 8 audit events, got {}: {:?}",
            events.len(),
            *events
        );

        // Verify specific event names are present
        assert!(events.iter().any(|e| e == "enclave_registered"));
        assert!(events.iter().any(|e| e == "challenge_detected"));
        assert!(events.iter().any(|e| e == "result_submitted"));
        assert!(events.iter().any(|e| e == "dispute_submitted"));
        assert!(events.iter().any(|e| e == "dispute_failed"));
        assert!(events.iter().any(|e| e == "config_loaded"));
        assert!(events.iter().any(|e| e == "result_finalized"));
        assert!(events.iter().any(|e| e == "prover_slashed"));
    }

    /// A minimal tracing layer that captures event names from the "audit" target.
    struct TestAuditLayer {
        captured: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TestAuditLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let meta = event.metadata();
            if meta.target() == "audit" {
                // Extract the "event" field value
                let mut visitor = EventFieldVisitor { event_name: None };
                event.record(&mut visitor);
                if let Some(name) = visitor.event_name {
                    self.captured.lock().unwrap().push(name);
                }
            }
        }
    }

    /// Visitor that extracts the "event" field from a tracing event.
    struct EventFieldVisitor {
        event_name: Option<String>,
    }

    impl tracing::field::Visit for EventFieldVisitor {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "event" {
                self.event_name = Some(value.to_string());
            }
        }

        fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
    }
}
