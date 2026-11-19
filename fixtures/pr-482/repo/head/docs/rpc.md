# RPC calls

Every call goes through `RpcClient`. The default timeout is 5 s per attempt.

## Sending

- `send` sends a request; a request that times out is sent again, up to 3 attempts in total.

## Timeouts

A request that gets no answer within the timeout is retried with exponential backoff. When no attempt is
left, the call fails with `RpcError::Exhausted { attempts }`.
