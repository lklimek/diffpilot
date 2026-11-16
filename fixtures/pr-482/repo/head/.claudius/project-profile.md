# Project profile: acme/node-client

- Kind: client library for the acme node RPC API.
- Users: the acme wallet (balances, payments) and the relay service (transaction broadcast, header sync).
- Critical paths: payments and transaction broadcast move value and must not be sent twice; header sync keeps
  wallets current.
- Scale: every wallet and relay instance runs this client; a change to `RpcClient::send` reaches all of them.
- Out of scope: node-side behaviour, which lives in another repository.
