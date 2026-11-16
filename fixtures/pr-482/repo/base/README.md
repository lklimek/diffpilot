# node-client

Client for the acme node RPC API: requests, wallet payments, mempool broadcasts and header sync.

## Usage

```rust
let client = RpcClient::new(RpcConfig::default(), Arc::new(NoAuth));
let resp = client.send(Request::get_headers(0, 100)).await?;
```

## Errors

`RpcError` is `#[non_exhaustive]`: keep a wildcard arm when matching on it.
