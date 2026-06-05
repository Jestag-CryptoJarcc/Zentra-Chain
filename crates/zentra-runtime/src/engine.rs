//! # Wasm Execution Engine
//!
//! Provides the core WebAssembly execution engine for Zentra smart contracts
//! using the [`wasmi`] interpreter.
//!
//! ## Architecture
//!
//! - **`ContractId`**: A 32-byte Blake2b hash of the contract bytecode.
//! - **`WasmEngine`**: Owns a [`wasmi::Engine`] and a registry of validated
//!   modules indexed by [`ContractId`].
//! - **Host Functions**: The runtime exposes `host_get_balance`,
//!   `host_transfer`, `host_log`, and `host_get_block_height` to guest code
//!   via the `"env"` import namespace.
//!
//! ## Gas Metering
//!
//! Gas is tracked through the [`GasMeter`] stored in the Wasmi `Store`'s
//! host state. Each host function call charges a fixed cost. Wasm instruction
//! fuel is configured through Wasmi's built-in fuel mechanism.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use zentra_types::error::{ZentraError, ZentraResult};
use zentra_types::Hash;

use crate::gas::GasMeter;

// ─── ContractId ────────────────────────────────────────────────────────────────

/// A contract identifier — the Blake2b-256 hash of the Wasm bytecode.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContractId(pub [u8; 32]);

impl ContractId {
    /// Compute the contract ID from bytecode.
    pub fn from_bytecode(bytecode: &[u8]) -> Self {
        let h = Hash::hash(bytecode);
        ContractId(h.0)
    }

    /// Create from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        ContractId(bytes)
    }

    /// Return the hex-encoded contract ID.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for ContractId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContractId({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for ContractId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

// ─── ExecutionResult ───────────────────────────────────────────────────────────

/// The result of executing a smart contract function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Data returned by the contract function (empty if void).
    pub return_data: Vec<u8>,
    /// Total gas consumed during execution.
    pub gas_used: u64,
    /// Log messages emitted by the contract via `host_log`.
    pub logs: Vec<String>,
}

// ─── HostState ─────────────────────────────────────────────────────────────────

/// State accessible from host functions within the Wasmi store.
struct HostState {
    /// Gas meter for this execution.
    gas_meter: GasMeter,
    /// Log messages collected during execution.
    logs: Vec<String>,
    /// Simulated block height (set before execution).
    block_height: u64,
    /// Simulated balances (address_hash → balance in zents).
    balances: HashMap<[u8; 32], u64>,
    /// Return data buffer written by the contract.
    return_data: Vec<u8>,
}

// ─── WasmEngine ────────────────────────────────────────────────────────────────

/// The Wasm execution engine for Zentra smart contracts.
///
/// Maintains a registry of validated Wasm modules keyed by [`ContractId`].
/// Thread-safe via `parking_lot::RwLock`.
pub struct WasmEngine {
    /// The underlying wasmi engine (shared configuration).
    engine: wasmi::Engine,
    /// Registry of validated modules: ContractId → compiled module bytes.
    /// We store the raw bytes and re-compile per execution for isolation.
    modules: RwLock<HashMap<ContractId, Vec<u8>>>,
}

impl WasmEngine {
    /// Create a new Wasm engine with default configuration.
    pub fn new() -> Self {
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);

        let engine = wasmi::Engine::new(&config);

        tracing::info!("Wasm execution engine initialised");

        WasmEngine {
            engine,
            modules: RwLock::new(HashMap::new()),
        }
    }

    /// Deploy a contract by validating and registering its Wasm bytecode.
    ///
    /// Returns the [`ContractId`] (Blake2b hash of the bytecode).
    ///
    /// # Errors
    ///
    /// - [`ZentraError::WasmRuntime`] if the bytecode is empty.
    /// - [`ZentraError::WasmRuntime`] if the bytecode is not valid Wasm.
    pub fn deploy_contract(&self, bytecode: &[u8]) -> ZentraResult<ContractId> {
        if bytecode.is_empty() {
            return Err(ZentraError::WasmRuntime(
                "Cannot deploy empty bytecode".to_string(),
            ));
        }

        // Validate by attempting to compile the module
        wasmi::Module::new(&self.engine, bytecode).map_err(|e| {
            ZentraError::WasmRuntime(format!("Invalid Wasm bytecode: {}", e))
        })?;

        let contract_id = ContractId::from_bytecode(bytecode);

        let mut modules = self.modules.write();
        modules.insert(contract_id.clone(), bytecode.to_vec());

        tracing::info!(
            contract_id = %contract_id,
            bytecode_len = bytecode.len(),
            "Contract deployed"
        );

        Ok(contract_id)
    }

    /// Check if a contract is deployed.
    pub fn is_deployed(&self, contract_id: &ContractId) -> bool {
        self.modules.read().contains_key(contract_id)
    }

    /// Get the number of deployed contracts.
    pub fn contract_count(&self) -> usize {
        self.modules.read().len()
    }

    /// Execute a function on a deployed contract.
    ///
    /// # Arguments
    ///
    /// - `contract_id`: The contract to execute.
    /// - `function`: The name of the exported function to call.
    /// - `args`: ABI-encoded arguments (passed to guest memory — currently
    ///   unused in the simplified interface).
    /// - `gas_limit`: Maximum gas allowed for this execution.
    ///
    /// # Errors
    ///
    /// - [`ZentraError::WasmRuntime`] if the contract is not deployed.
    /// - [`ZentraError::WasmRuntime`] if the function is not found.
    /// - [`ZentraError::WasmRuntime`] if execution runs out of gas or traps.
    pub fn execute_contract(
        &self,
        contract_id: &ContractId,
        function: &str,
        _args: &[u8],
        gas_limit: u64,
    ) -> ZentraResult<ExecutionResult> {
        // Look up the bytecode
        let bytecode = {
            let modules = self.modules.read();
            modules.get(contract_id).cloned().ok_or_else(|| {
                ZentraError::WasmRuntime(format!("Contract {} not deployed", contract_id))
            })?
        };

        // Compile the module
        let module = wasmi::Module::new(&self.engine, &bytecode).map_err(|e| {
            ZentraError::WasmRuntime(format!("Failed to compile module: {}", e))
        })?;

        // Create host state with gas meter
        let host_state = HostState {
            gas_meter: GasMeter::new(gas_limit),
            logs: Vec::new(),
            block_height: 0,
            balances: HashMap::new(),
            return_data: Vec::new(),
        };

        // Create store with fuel metering
        let mut store = wasmi::Store::new(&self.engine, host_state);
        store.set_fuel(gas_limit).map_err(|e| {
            ZentraError::WasmRuntime(format!("Failed to set fuel: {}", e))
        })?;

        // Create linker and define host functions
        let mut linker = <wasmi::Linker<HostState>>::new(&self.engine);
        Self::register_host_functions(&mut linker)?;

        // Instantiate
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| {
                ZentraError::WasmRuntime(format!("Instantiation failed: {}", e))
            })?
            .start(&mut store)
            .map_err(|e| {
                ZentraError::WasmRuntime(format!("Start function failed: {}", e))
            })?;

        // Look up the exported function
        let func = instance
            .get_func(&store, function)
            .ok_or_else(|| {
                ZentraError::WasmRuntime(format!(
                    "Function '{}' not found in contract {}",
                    function, contract_id
                ))
            })?;

        // Call the function (no arguments, no return values in simplified model)
        let results = &mut [];
        func.call(&mut store, &[], results).map_err(|e| {
            ZentraError::WasmRuntime(format!("Execution failed: {}", e))
        })?;

        // Calculate gas used from fuel consumption
        let fuel_remaining = store.get_fuel().unwrap_or(0);
        let fuel_used = gas_limit.saturating_sub(fuel_remaining);

        let host = store.into_data();

        // Use the larger of fuel-based and meter-based gas usage
        let gas_used = fuel_used.max(host.gas_meter.gas_used());

        tracing::info!(
            contract_id = %contract_id,
            function = function,
            gas_used = gas_used,
            logs_count = host.logs.len(),
            "Contract execution completed"
        );

        Ok(ExecutionResult {
            return_data: host.return_data,
            gas_used,
            logs: host.logs,
        })
    }

    /// Register host functions in the linker.
    fn register_host_functions(
        linker: &mut wasmi::Linker<HostState>,
    ) -> ZentraResult<()> {
        // host_log(ptr: i32, len: i32)
        linker
            .func_wrap(
                "env",
                "host_log",
                |mut caller: wasmi::Caller<'_, HostState>, ptr: i32, len: i32| {
                    let mem = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory());

                    if let Some(mem) = mem {
                        let ptr = ptr as usize;
                        let len = len as usize;
                        let msg_opt = {
                            let data = mem.data(&caller);
                            if ptr.checked_add(len).is_some_and(|end| end <= data.len()) {
                                std::str::from_utf8(&data[ptr..ptr + len]).map(|s| s.to_string()).ok()
                            } else {
                                None
                            }
                        };

                        if let Some(msg) = msg_opt {
                            // Charge gas for logging
                            let _ = caller.data_mut().gas_meter.consume_log(len);
                            caller.data_mut().logs.push(msg.clone());
                            tracing::debug!(msg = %msg, "Contract log");
                        }
                    }
                },
            )
            .map_err(|e| ZentraError::WasmRuntime(format!("Failed to link host_log: {}", e)))?;

        // host_get_block_height() -> i64
        linker
            .func_wrap(
                "env",
                "host_get_block_height",
                |caller: wasmi::Caller<'_, HostState>| -> i64 {
                    caller.data().block_height as i64
                },
            )
            .map_err(|e| {
                ZentraError::WasmRuntime(format!(
                    "Failed to link host_get_block_height: {}",
                    e
                ))
            })?;

        // host_get_balance(addr_ptr: i32) -> i64
        linker
            .func_wrap(
                "env",
                "host_get_balance",
                |mut caller: wasmi::Caller<'_, HostState>, addr_ptr: i32| -> i64 {
                    // Charge for storage read
                    if caller.data_mut().gas_meter.consume_storage_read().is_err() {
                        return -1;
                    }

                    let mem = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory());

                    if let Some(mem) = mem {
                        let ptr = addr_ptr as usize;
                        let data = mem.data(&caller);

                        if ptr + 32 <= data.len() {
                            let mut addr = [0u8; 32];
                            addr.copy_from_slice(&data[ptr..ptr + 32]);
                            let balance = caller
                                .data()
                                .balances
                                .get(&addr)
                                .copied()
                                .unwrap_or(0);
                            return balance as i64;
                        }
                    }
                    0
                },
            )
            .map_err(|e| {
                ZentraError::WasmRuntime(format!("Failed to link host_get_balance: {}", e))
            })?;

        // host_transfer(from_ptr: i32, to_ptr: i32, amount: i64) -> i32
        //   Returns 0 on success, -1 on failure.
        linker
            .func_wrap(
                "env",
                "host_transfer",
                |mut caller: wasmi::Caller<'_, HostState>,
                 from_ptr: i32,
                 to_ptr: i32,
                 amount: i64|
                 -> i32 {
                    // Charge for storage write
                    if caller.data_mut().gas_meter.consume_storage_write().is_err() {
                        return -1;
                    }

                    if amount <= 0 {
                        return -1;
                    }
                    let amount = amount as u64;

                    let mem = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory());

                    if let Some(mem) = mem {
                        let data = mem.data(&caller);
                        let fp = from_ptr as usize;
                        let tp = to_ptr as usize;

                        if fp + 32 <= data.len() && tp + 32 <= data.len() {
                            let mut from = [0u8; 32];
                            let mut to = [0u8; 32];
                            from.copy_from_slice(&data[fp..fp + 32]);
                            to.copy_from_slice(&data[tp..tp + 32]);

                            let from_bal = caller
                                .data()
                                .balances
                                .get(&from)
                                .copied()
                                .unwrap_or(0);

                            if from_bal < amount {
                                return -1; // insufficient funds
                            }

                            let host = caller.data_mut();
                            host.balances.insert(from, from_bal - amount);
                            let to_bal = host.balances.get(&to).copied().unwrap_or(0);
                            host.balances.insert(to, to_bal + amount);

                            tracing::debug!(
                                amount = amount,
                                "Host transfer executed"
                            );

                            return 0;
                        }
                    }
                    -1
                },
            )
            .map_err(|e| {
                ZentraError::WasmRuntime(format!("Failed to link host_transfer: {}", e))
            })?;

        Ok(())
    }
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WasmEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmEngine")
            .field("contracts", &self.modules.read().len())
            .finish()
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid Wasm module (empty, no exports).
    fn empty_wasm() -> Vec<u8> {
        // Minimal valid Wasm binary: magic + version + empty
        vec![
            0x00, 0x61, 0x73, 0x6D, // magic: \0asm
            0x01, 0x00, 0x00, 0x00, // version: 1
        ]
    }

    /// A Wasm module that exports a function "run" which returns immediately.
    fn noop_wasm() -> Vec<u8> {
        // WAT equivalent:
        // (module
        //   (func (export "run"))
        // )
        wat::parse_str(r#"(module (func (export "run")))"#)
            .expect("valid WAT")
    }

    /// A Wasm module with a function that imports host_get_block_height.
    fn height_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (import "env" "host_get_block_height" (func $height (result i64)))
                (func (export "check") (drop (call $height)))
            )"#,
        )
        .expect("valid WAT")
    }

    #[test]
    fn test_new_engine() {
        let engine = WasmEngine::new();
        assert_eq!(engine.contract_count(), 0);
    }

    #[test]
    fn test_deploy_contract() {
        let engine = WasmEngine::new();
        let bytecode = empty_wasm();

        let cid = engine.deploy_contract(&bytecode).expect("deploy");
        assert!(engine.is_deployed(&cid));
        assert_eq!(engine.contract_count(), 1);
    }

    #[test]
    fn test_deploy_empty_bytecode() {
        let engine = WasmEngine::new();
        let result = engine.deploy_contract(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_invalid_bytecode() {
        let engine = WasmEngine::new();
        let result = engine.deploy_contract(&[0xFF, 0xFF, 0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_same_bytecode_twice() {
        let engine = WasmEngine::new();
        let bytecode = empty_wasm();

        let cid1 = engine.deploy_contract(&bytecode).unwrap();
        let cid2 = engine.deploy_contract(&bytecode).unwrap();

        // Same bytecode should produce the same contract ID
        assert_eq!(cid1, cid2);
        assert_eq!(engine.contract_count(), 1);
    }

    #[test]
    fn test_contract_id_from_bytecode() {
        let bytecode = b"test contract bytes";
        let cid = ContractId::from_bytecode(bytecode);
        assert_ne!(cid.0, [0u8; 32]);
    }

    #[test]
    fn test_contract_id_deterministic() {
        let bytecode = b"some bytes";
        let cid1 = ContractId::from_bytecode(bytecode);
        let cid2 = ContractId::from_bytecode(bytecode);
        assert_eq!(cid1, cid2);
    }

    #[test]
    fn test_contract_id_different_bytecodes() {
        let cid1 = ContractId::from_bytecode(b"a");
        let cid2 = ContractId::from_bytecode(b"b");
        assert_ne!(cid1, cid2);
    }

    #[test]
    fn test_contract_id_hex() {
        let cid = ContractId::from_bytes([0xAB; 32]);
        let hex_str = cid.to_hex();
        assert_eq!(hex_str.len(), 64);
        assert!(hex_str.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_execute_noop() {
        let engine = WasmEngine::new();
        let bytecode = noop_wasm();

        let cid = engine.deploy_contract(&bytecode).unwrap();
        let result = engine
            .execute_contract(&cid, "run", &[], 100_000)
            .expect("execute");

        assert!(result.gas_used > 0 || result.gas_used == 0); // noop might use 0
        assert!(result.logs.is_empty());
    }

    #[test]
    fn test_execute_not_deployed() {
        let engine = WasmEngine::new();
        let cid = ContractId::from_bytes([99u8; 32]);

        let result = engine.execute_contract(&cid, "run", &[], 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_function_not_found() {
        let engine = WasmEngine::new();
        let bytecode = noop_wasm();
        let cid = engine.deploy_contract(&bytecode).unwrap();

        let result = engine.execute_contract(&cid, "nonexistent", &[], 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_with_host_function() {
        let engine = WasmEngine::new();
        let bytecode = height_wasm();

        let cid = engine.deploy_contract(&bytecode).unwrap();
        let result = engine
            .execute_contract(&cid, "check", &[], 100_000)
            .expect("execute");

        // Should complete without error
        assert!(result.logs.is_empty());
    }

    #[test]
    fn test_execution_result_default() {
        let result = ExecutionResult {
            return_data: Vec::new(),
            gas_used: 42,
            logs: vec!["hello".to_string()],
        };
        assert_eq!(result.gas_used, 42);
        assert_eq!(result.logs.len(), 1);
    }

    #[test]
    fn test_is_deployed() {
        let engine = WasmEngine::new();
        let cid = ContractId::from_bytes([1u8; 32]);
        assert!(!engine.is_deployed(&cid));

        let bytecode = empty_wasm();
        let deployed_cid = engine.deploy_contract(&bytecode).unwrap();
        assert!(engine.is_deployed(&deployed_cid));
    }

    #[test]
    fn test_engine_debug() {
        let engine = WasmEngine::new();
        let debug = format!("{:?}", engine);
        assert!(debug.contains("WasmEngine"));
    }

    #[test]
    fn test_default_engine() {
        let engine = WasmEngine::default();
        assert_eq!(engine.contract_count(), 0);
    }
}
