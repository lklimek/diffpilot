# RPC calls

Every call goes through `RpcClient`. The default timeout is 30 s per request.

## Sending

- `send` sends a request and returns the first answer or error.
- `send_once` sends a request exactly once. Use it for calls that must not be repeated, such as
  `broadcast_tx`.

## Timeouts

A request that gets no answer within the timeout fails with `RpcError::Timeout`.
