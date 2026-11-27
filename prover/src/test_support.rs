//! Helpers shared by unit tests

use crate::contracts::IExecutionEngine;
use crate::monitor::{signing_provider, SigningNonceManager, SigningProvider};
use crate::nonce::NonceManager;
use alloy::consensus::{Transaction, TxEnvelope};
use alloy::eips::eip2718::Decodable2718;
use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::{SolCall, SolEvent, SolValue};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request received by [`serve_http`]
pub struct HttpRequest {
    pub path: String,
    pub body: Vec<u8>,
}

/// Serve HTTP on a free loopback port. `handler` maps each request to a
/// status code and a response body. The server runs until the runtime of the
/// calling test shuts down.
pub async fn serve_http<F>(handler: F) -> SocketAddr
where
    F: Fn(&HttpRequest) -> (u16, Vec<u8>) + Send + Sync + 'static,
{
    serve_http_late(move |request| {
        let (status, body) = handler(request);
        (status, body, Duration::ZERO)
    })
    .await
}

/// [`serve_http`] with a handler that also says how long to wait before
/// the answer is written
async fn serve_http_late<F>(handler: F) -> SocketAddr
where
    F: Fn(&HttpRequest) -> (u16, Vec<u8>, Duration) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_connection(stream, handler.clone()));
        }
    });

    addr
}

/// Answer requests on one connection until the client closes it
async fn serve_connection<F>(mut stream: TcpStream, handler: Arc<F>)
where
    F: Fn(&HttpRequest) -> (u16, Vec<u8>, Duration) + Send + Sync + 'static,
{
    let mut buf = Vec::new();

    loop {
        // Request line and headers
        let head_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            if !read_more(&mut stream, &mut buf).await {
                return;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or_default();
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);

        // Body
        while buf.len() < head_end + content_length {
            if !read_more(&mut stream, &mut buf).await {
                return;
            }
        }
        let body = buf[head_end..head_end + content_length].to_vec();
        buf.drain(..head_end + content_length);

        let (status, response_body, delay) = handler(&HttpRequest { path, body });
        tokio::time::sleep(delay).await;
        let reason = if status == 200 { "OK" } else { "Error" };
        let mut response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            status,
            reason,
            response_body.len()
        )
        .into_bytes();
        response.extend_from_slice(&response_body);
        if stream.write_all(&response).await.is_err() {
            return;
        }
    }
}

/// Read more bytes into `buf`; false when the client is gone
async fn read_more(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}

/// Block number of the latest block of a [`MockChain`]
const LATEST_BLOCK: u64 = 100;

/// How late a [`MockNode`] answers the calls of
/// [`MockChain::late_methods`]: longer than the request timeout tests give
/// [`MockNode::start_with`]
pub const LATE_ANSWER_DELAY: Duration = Duration::from_secs(2);

/// A transaction accepted by a [`MockNode`]
pub struct MockTransaction {
    pub nonce: u64,
    pub hash: B256,
}

/// The state of a [`MockNode`] and how it answers
pub struct MockChain {
    /// Methods called
    pub calls: Vec<String>,
    /// Transaction count of the account at the latest block
    pub mined_count: u64,
    /// Transaction count of the account including pending transactions: the
    /// lowest nonce that is accepted
    pub pending_count: u64,
    /// Accepted transactions, in order
    pub transactions: Vec<MockTransaction>,
    /// Accepted transactions are forgotten: they have no receipt and do not
    /// count
    pub drops_transactions: bool,
    /// Status in receipts; None: there are no receipts
    pub receipt_status: Option<&'static str>,
    /// Requests whose transactions revert at gas estimation
    pub reverting_requests: Vec<U256>,
    /// Block numbers and events of the engine
    pub events: Vec<(u64, IExecutionEngine::ExecutionRequested)>,
    /// eth_getLogs fails
    pub fail_get_logs: bool,
    /// eth_getTransactionCount fails
    pub fail_transaction_count: bool,
    /// Number of eth_getTransactionReceipt calls that fail before the
    /// receipts are given
    pub failing_receipt_reads: usize,
    /// Methods whose calls are answered [`LATE_ANSWER_DELAY`] late. The call
    /// takes effect at once: a late eth_sendRawTransaction is accepted.
    pub late_methods: Vec<&'static str>,
}

impl Default for MockChain {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            mined_count: 0,
            pending_count: 0,
            transactions: Vec::new(),
            drops_transactions: false,
            receipt_status: Some("0x1"),
            reverting_requests: Vec::new(),
            events: Vec::new(),
            fail_get_logs: false,
            fail_transaction_count: false,
            failing_receipt_reads: 0,
            late_methods: Vec::new(),
        }
    }
}

impl MockChain {
    /// Answer a JSON-RPC call with a result or an error code and message
    fn answer(&mut self, method: &str, params: &Value) -> Result<Value, (i64, &'static str)> {
        self.calls.push(method.to_string());

        match method {
            "eth_chainId" => Ok(json!("0x7a69")),
            "eth_blockNumber" => Ok(json!(format!("{LATEST_BLOCK:#x}"))),
            "eth_feeHistory" => Ok(json!({
                "oldestBlock": "0x1",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca00"],
                "gasUsedRatio": [0.5],
                "reward": [["0x3b9aca00"]],
            })),
            "eth_getTransactionCount" => {
                if self.fail_transaction_count {
                    return Err((-32603, "internal error"));
                }
                let count = match params[1].as_str() {
                    Some("pending") => self.pending_count,
                    _ => self.mined_count,
                };
                Ok(json!(format!("{count:#x}")))
            }
            "eth_estimateGas" => {
                let call: TransactionRequest = serde_json::from_value(params[0].clone()).unwrap();
                // The first argument of every engine call is the request ID
                let calldata = call.input.input().unwrap();
                let request_id = U256::from_be_slice(&calldata[4..36]);
                if self.reverting_requests.contains(&request_id) {
                    return Err((3, "execution reverted"));
                }
                Ok(json!("0x186a0"))
            }
            "eth_sendRawTransaction" => self.accept(params[0].as_str().unwrap()),
            "eth_getTransactionReceipt" => {
                if self.failing_receipt_reads > 0 {
                    self.failing_receipt_reads -= 1;
                    return Err((-32005, "rate limit exceeded"));
                }
                Ok(self.receipt(params[0].as_str().unwrap()))
            }
            "eth_getLogs" => {
                if self.fail_get_logs {
                    return Err((-32005, "query returned more than 10000 results"));
                }
                Ok(self.logs(&params[0]))
            }
            "eth_call" => Ok(self.read_engine(&params[0])),
            _ => Err((-32601, "method not found")),
        }
    }

    fn accept(&mut self, raw: &str) -> Result<Value, (i64, &'static str)> {
        let raw = hex::decode(raw.trim_start_matches("0x")).unwrap();
        let tx = TxEnvelope::decode_2718(&mut raw.as_slice()).unwrap();
        if tx.nonce() < self.pending_count {
            return Err((-32000, "nonce too low"));
        }

        let hash = *tx.tx_hash();
        // A transaction behind a gap waits in the pool and does not count
        if tx.nonce() == self.pending_count && !self.drops_transactions {
            self.pending_count += 1;
        }
        self.transactions.push(MockTransaction {
            nonce: tx.nonce(),
            hash,
        });
        Ok(json!(hash))
    }

    /// Transactions that count have a receipt
    fn receipt(&self, hash: &str) -> Value {
        let hash: B256 = hash.parse().unwrap();
        let tx = self.transactions.iter().find(|tx| tx.hash == hash);
        match (tx, self.receipt_status) {
            (Some(tx), Some(status)) if tx.nonce < self.pending_count => json!({
                "transactionHash": hash,
                "transactionIndex": "0x0",
                "blockHash": hash,
                "blockNumber": "0x1",
                "from": format!("0x{}", "22".repeat(20)),
                "to": format!("0x{}", "ee".repeat(20)),
                "cumulativeGasUsed": "0x186a0",
                "gasUsed": "0x186a0",
                "effectiveGasPrice": "0x3b9aca00",
                "contractAddress": null,
                "logs": [],
                "logsBloom": format!("0x{}", "00".repeat(256)),
                "status": status,
                "type": "0x2",
            }),
            _ => Value::Null,
        }
    }

    /// Events within the block range of the filter that are about one of its
    /// request IDs. Without a range the latest block is searched.
    fn logs(&self, filter: &Value) -> Value {
        let block = |field: &str| match filter[field].as_str() {
            None | Some("latest") => LATEST_BLOCK,
            Some("earliest") => 0,
            Some(number) => u64::from_str_radix(number.trim_start_matches("0x"), 16).unwrap(),
        };
        let range = block("fromBlock")..=block("toBlock");
        let wanted: Vec<B256> = match &filter["topics"][1] {
            Value::Array(ids) => ids
                .iter()
                .map(|id| serde_json::from_value(id.clone()).unwrap())
                .collect(),
            id => vec![serde_json::from_value(id.clone()).unwrap()],
        };

        let mut logs = Vec::new();
        for (block_number, event) in &self.events {
            if !range.contains(block_number) || !wanted.contains(&event.requestId.into()) {
                continue;
            }
            let data = event.encode_log_data();
            logs.push(json!({
                "address": filter["address"],
                "topics": data.topics(),
                "data": data.data,
                "blockNumber": format!("{block_number:#x}"),
                "blockHash": B256::repeat_byte(0x66),
                "transactionHash": B256::repeat_byte(0x77),
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "removed": false,
            }));
        }
        json!(logs)
    }

    /// Any request is pending and has the same tip
    fn read_engine(&self, call: &Value) -> Value {
        let call: TransactionRequest = serde_json::from_value(call.clone()).unwrap();
        let calldata = call.input.input().unwrap();
        let tip = U256::from(1_000_000_000_000_000u64);

        let result = match calldata[..4].try_into().unwrap() {
            IExecutionEngine::getCurrentTipCall::SELECTOR => tip.abi_encode(),
            IExecutionEngine::getRequestCall::SELECTOR => IExecutionEngine::ExecutionRequest {
                id: U256::from_be_slice(&calldata[4..36]),
                imageId: B256::repeat_byte(0x44),
                inputDigest: B256::repeat_byte(0x55),
                requester: Address::repeat_byte(0x33),
                createdAt: Default::default(),
                expiresAt: alloy::primitives::Uint::MAX,
                callbackContract: Address::ZERO,
                status: 0,
                claimedBy: Address::ZERO,
                claimedAt: Default::default(),
                claimDeadline: Default::default(),
                tip,
                maxTip: tip,
            }
            .abi_encode(),
            selector => panic!("unexpected call {selector:02x?}"),
        };
        json!(format!("0x{}", hex::encode(result)))
    }
}

/// Stand-in for a JSON-RPC node with a [`MockChain`], and a provider and a
/// nonce manager that use it
pub struct MockNode {
    pub provider: Arc<SigningProvider>,
    pub nonce_manager: Arc<SigningNonceManager>,
    chain: Arc<Mutex<MockChain>>,
}

impl MockNode {
    pub async fn start(chain: MockChain) -> Self {
        Self::start_with(chain, "", Duration::from_secs(60)).await
    }

    /// A node whose URL has the path `path`, with a provider that gives up
    /// on a request after `request_timeout`
    pub async fn start_with(chain: MockChain, path: &str, request_timeout: Duration) -> Self {
        let chain = Arc::new(Mutex::new(chain));

        let served = chain.clone();
        let addr = serve_http_late(move |request| {
            let call: Value = serde_json::from_slice(&request.body).unwrap();
            let method = call["method"].as_str().unwrap();
            let mut chain = served.lock().unwrap();
            let delay = if chain.late_methods.contains(&method) {
                LATE_ANSWER_DELAY
            } else {
                Duration::ZERO
            };
            let response = match chain.answer(method, &call["params"]) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": call["id"], "result": result }),
                Err((code, message)) => json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "error": { "code": code, "message": message },
                }),
            };
            (200, response.to_string().into_bytes(), delay)
        })
        .await;

        // Same provider as in run_prover
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let provider = signing_provider(
            format!("http://{addr}{path}").parse().unwrap(),
            signer,
            Duration::from_secs(30),
            request_timeout,
        )
        .unwrap();
        let provider = Arc::new(provider);
        let nonce_manager = Arc::new(NonceManager::new(provider.clone(), address).await.unwrap());

        Self {
            provider,
            nonce_manager,
            chain,
        }
    }

    /// The state of the node; calls to the node wait while it is held
    pub fn chain(&self) -> MutexGuard<'_, MockChain> {
        self.chain.lock().unwrap()
    }

    pub fn engine(&self) -> IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>> {
        IExecutionEngine::new(Address::repeat_byte(0xee), self.provider.clone())
    }

    /// Number of transactions sent to the node
    pub fn transactions_sent(&self) -> usize {
        let chain = self.chain();
        chain
            .calls
            .iter()
            .filter(|method| *method == "eth_sendRawTransaction")
            .count()
    }

    /// Nonces of the transactions the node accepted, in order
    pub fn accepted_nonces(&self) -> Vec<u64> {
        let chain = self.chain();
        chain.transactions.iter().map(|tx| tx.nonce).collect()
    }
}
