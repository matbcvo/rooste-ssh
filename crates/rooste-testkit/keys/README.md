# Test-only keys

These keys exist only for automated tests. They are public, so **never** use them anywhere else.

| File | Purpose |
|---|---|
| `id_ed25519` | Ed25519 client key |
| `id_ed25519_encrypted` | Ed25519 client key, passphrase `rooste` |
| `id_rsa` | RSA 3072 client key |
| `host_ed25519` | Host key of the in-process test server |

The client public keys are also in `../docker/authorized_keys`.
