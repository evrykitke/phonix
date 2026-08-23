# phonix-messaging — RabbitMQ

![architecture](../../docs/architecture.svg)

Topology, declared idempotently at startup so the broker's data volume is never
the source of truth:

```text
  phonix.events (topic)  --routing key: tenant.<slug>.<aggregate>.<event>
       |
       +--> phonix.tenant-events   (durable, DLX -> phonix.dlx)

  phonix.dlx (fanout) --> phonix.dlq
```

Every message carries its tenant **twice** — in the routing key and in an
`x-tenant` header — so a consumer can pick the right database without parsing
the key, and a mis-parsed key cannot silently route work to the wrong workspace.

## How it connects

```text
phonix-server ──> phonix-messaging ──> RabbitMQ
phonix-web ─────> phonix-messaging        (through AppState)

phonix-messaging ──> phonix-core, phonix-config
```

Cross-cutting: available to every layer, dependent on none of them.
