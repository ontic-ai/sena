# Crate: crypto
Path: crates/crypto/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
Encryption primitives, key management, and file encryption for Sena. Provides envelope encryption with AES-256-GCM, OS keychain integration via keyring, and Argon2id passphrase derivation as fallback. This is a leaf node with zero Sena crate dependencies.

## Public API Surface
**Types:**
- `MasterKey` — 32-byte master key (ZeroizeOnDrop, Debug redacted)
- `DEK` — Data Encryption Key (per-file)
- `CryptoError` — error enum

**Modules:**
- `aes` — AES-256-GCM encrypt/decrypt
- `argon2_kdf` — Argon2id key derivation from passphrase
- `envelope` — envelope encryption primitives
- `error` — error types
- `file` — encrypted file operations
- `keychain` — OS keychain store/retrieve
- `keys` — key types (MasterKey, DEK)
- `reencrypt` — re-encryption on passphrase change
- `working_file` — encrypted file handles

## Bus Events Owned
None — crypto is a service crate, not an actor

## Dependency Edges
Imports from Sena crates: (none — leaf node)
Imported by Sena crates: runtime, soul, memory
Key external deps:
- zeroize (v1.8, features = ["derive"]) — secure memory wiping
- rand (v0.9) — nonce/salt generation
- aes-gcm (v0.10) — AES-256-GCM
- argon2 (v0.5) — key derivation
- keyring (v3) — OS keychain (platform-specific features)

## Background Loops Owned
None

## Known Issues
None

## Notes
- Master key NEVER written to disk
- DEK NEVER written to disk unencrypted
- All key types implement `ZeroizeOnDrop`
- Custom Debug impls redact sensitive content
- Nonces fresh per encryption via rand
- Platform-specific keyring features enabled per OS
