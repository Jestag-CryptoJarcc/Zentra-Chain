//! # Gas Metering
//!
//! Provides a simple, deterministic gas meter for smart-contract execution.
//! Every Wasm operation, storage access, and cryptographic primitive is
//! assigned a fixed gas cost. The meter tracks cumulative usage and fails
//! with [`ZentraError::WasmRuntime`] when the limit is exceeded.

use zentra_types::error::{ZentraError, ZentraResult};

// ─── Gas Cost Constants ────────────────────────────────────────────────────────

/// Gas consumed per Wasm instruction executed.
pub const GAS_PER_WASM_INSTRUCTION: u64 = 1;

/// Gas consumed per storage read (key lookup).
pub const GAS_PER_STORAGE_READ: u64 = 100;

/// Gas consumed per storage write (key insert / update / delete).
pub const GAS_PER_STORAGE_WRITE: u64 = 500;

/// Gas consumed per hash computation (Blake2b-256).
pub const GAS_PER_HASH: u64 = 50;

/// Gas consumed per Ed25519 signature verification.
pub const GAS_PER_SIGNATURE_VERIFY: u64 = 1_000;

/// Gas consumed per byte of log output emitted by a contract.
pub const GAS_PER_LOG_BYTE: u64 = 2;

/// Gas consumed per byte of return data.
pub const GAS_PER_RETURN_BYTE: u64 = 1;

/// Gas consumed for a cross-contract call setup.
pub const GAS_PER_CALL_SETUP: u64 = 500;

// ─── GasMeter ──────────────────────────────────────────────────────────────────

/// Tracks gas consumption for a single smart-contract execution.
///
/// Created with a `gas_limit` and monotonically increments `gas_used`.
/// Once gas is exhausted, every subsequent `consume` call returns an error.
#[derive(Debug, Clone)]
pub struct GasMeter {
    /// Maximum gas allowed for this execution.
    gas_limit: u64,
    /// Gas consumed so far.
    gas_used: u64,
}

impl GasMeter {
    /// Create a new meter with the given gas limit.
    pub fn new(limit: u64) -> Self {
        GasMeter {
            gas_limit: limit,
            gas_used: 0,
        }
    }

    /// Attempt to consume `amount` gas.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::WasmRuntime`] if the consumption would exceed
    /// the gas limit, without modifying `gas_used`.
    pub fn consume(&mut self, amount: u64) -> ZentraResult<()> {
        let new_total = self.gas_used.checked_add(amount).ok_or_else(|| {
            ZentraError::WasmRuntime(format!(
                "Gas overflow: used={}, requested={}",
                self.gas_used, amount
            ))
        })?;

        if new_total > self.gas_limit {
            Err(ZentraError::WasmRuntime(format!(
                "Out of gas: limit={}, used={}, requested={}",
                self.gas_limit, self.gas_used, amount
            )))
        } else {
            self.gas_used = new_total;
            Ok(())
        }
    }

    /// Return the amount of gas remaining.
    pub fn remaining(&self) -> u64 {
        self.gas_limit.saturating_sub(self.gas_used)
    }

    /// Return the amount of gas consumed so far.
    pub fn gas_used(&self) -> u64 {
        self.gas_used
    }

    /// Return the total gas limit.
    pub fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    /// Check whether the meter is fully exhausted.
    pub fn is_exhausted(&self) -> bool {
        self.gas_used >= self.gas_limit
    }

    /// Consume gas for a number of Wasm instructions.
    pub fn consume_instructions(&mut self, count: u64) -> ZentraResult<()> {
        self.consume(count.saturating_mul(GAS_PER_WASM_INSTRUCTION))
    }

    /// Consume gas for a storage read.
    pub fn consume_storage_read(&mut self) -> ZentraResult<()> {
        self.consume(GAS_PER_STORAGE_READ)
    }

    /// Consume gas for a storage write.
    pub fn consume_storage_write(&mut self) -> ZentraResult<()> {
        self.consume(GAS_PER_STORAGE_WRITE)
    }

    /// Consume gas for a hash computation.
    pub fn consume_hash(&mut self) -> ZentraResult<()> {
        self.consume(GAS_PER_HASH)
    }

    /// Consume gas for a signature verification.
    pub fn consume_signature_verify(&mut self) -> ZentraResult<()> {
        self.consume(GAS_PER_SIGNATURE_VERIFY)
    }

    /// Consume gas for emitting a log message of the given byte length.
    pub fn consume_log(&mut self, len: usize) -> ZentraResult<()> {
        self.consume((len as u64).saturating_mul(GAS_PER_LOG_BYTE))
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_meter() {
        let meter = GasMeter::new(1_000);
        assert_eq!(meter.gas_limit(), 1_000);
        assert_eq!(meter.gas_used(), 0);
        assert_eq!(meter.remaining(), 1_000);
        assert!(!meter.is_exhausted());
    }

    #[test]
    fn test_consume_success() {
        let mut meter = GasMeter::new(1_000);
        meter.consume(100).unwrap();
        assert_eq!(meter.gas_used(), 100);
        assert_eq!(meter.remaining(), 900);
    }

    #[test]
    fn test_consume_exact_limit() {
        let mut meter = GasMeter::new(500);
        meter.consume(500).unwrap();
        assert_eq!(meter.gas_used(), 500);
        assert_eq!(meter.remaining(), 0);
        assert!(meter.is_exhausted());
    }

    #[test]
    fn test_consume_exceeds_limit() {
        let mut meter = GasMeter::new(100);
        meter.consume(50).unwrap();
        let result = meter.consume(51);
        assert!(result.is_err());
        // gas_used should NOT have changed
        assert_eq!(meter.gas_used(), 50);
    }

    #[test]
    fn test_consume_after_exhausted() {
        let mut meter = GasMeter::new(10);
        meter.consume(10).unwrap();
        assert!(meter.is_exhausted());

        let result = meter.consume(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_consume_zero() {
        let mut meter = GasMeter::new(100);
        meter.consume(0).unwrap();
        assert_eq!(meter.gas_used(), 0);
    }

    #[test]
    fn test_consume_overflow_protection() {
        let mut meter = GasMeter::new(u64::MAX);
        meter.consume(u64::MAX).unwrap();
        // Consuming even 1 more should overflow-protect
        let result = meter.consume(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_consume_instructions() {
        let mut meter = GasMeter::new(100);
        meter.consume_instructions(10).unwrap();
        assert_eq!(meter.gas_used(), 10 * GAS_PER_WASM_INSTRUCTION);
    }

    #[test]
    fn test_consume_storage_read() {
        let mut meter = GasMeter::new(1_000);
        meter.consume_storage_read().unwrap();
        assert_eq!(meter.gas_used(), GAS_PER_STORAGE_READ);
    }

    #[test]
    fn test_consume_storage_write() {
        let mut meter = GasMeter::new(1_000);
        meter.consume_storage_write().unwrap();
        assert_eq!(meter.gas_used(), GAS_PER_STORAGE_WRITE);
    }

    #[test]
    fn test_consume_hash() {
        let mut meter = GasMeter::new(1_000);
        meter.consume_hash().unwrap();
        assert_eq!(meter.gas_used(), GAS_PER_HASH);
    }

    #[test]
    fn test_consume_signature_verify() {
        let mut meter = GasMeter::new(10_000);
        meter.consume_signature_verify().unwrap();
        assert_eq!(meter.gas_used(), GAS_PER_SIGNATURE_VERIFY);
    }

    #[test]
    fn test_consume_log() {
        let mut meter = GasMeter::new(10_000);
        meter.consume_log(100).unwrap();
        assert_eq!(meter.gas_used(), 100 * GAS_PER_LOG_BYTE);
    }

    #[test]
    fn test_mixed_operations() {
        let mut meter = GasMeter::new(10_000);

        meter.consume_instructions(100).unwrap(); // 100
        meter.consume_storage_read().unwrap(); // 100
        meter.consume_storage_write().unwrap(); // 500
        meter.consume_hash().unwrap(); // 50

        let expected = 100 * GAS_PER_WASM_INSTRUCTION
            + GAS_PER_STORAGE_READ
            + GAS_PER_STORAGE_WRITE
            + GAS_PER_HASH;
        assert_eq!(meter.gas_used(), expected);
        assert_eq!(meter.remaining(), 10_000 - expected);
    }

    #[test]
    fn test_zero_limit_meter() {
        let mut meter = GasMeter::new(0);
        assert!(meter.is_exhausted());
        assert!(meter.consume(1).is_err());
        // But consuming 0 should succeed
        meter.consume(0).unwrap();
    }

    #[test]
    fn test_gas_constants_positive() {
        assert!(GAS_PER_WASM_INSTRUCTION > 0);
        assert!(GAS_PER_STORAGE_READ > 0);
        assert!(GAS_PER_STORAGE_WRITE > 0);
        assert!(GAS_PER_HASH > 0);
        assert!(GAS_PER_SIGNATURE_VERIFY > 0);
        assert!(GAS_PER_LOG_BYTE > 0);
    }

    #[test]
    fn test_write_more_expensive_than_read() {
        assert!(
            GAS_PER_STORAGE_WRITE > GAS_PER_STORAGE_READ,
            "Storage writes should cost more than reads"
        );
    }
}
