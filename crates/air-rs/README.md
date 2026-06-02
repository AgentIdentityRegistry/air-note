# air-rs

Reference Rust implementation of the **AIR Agent-to-Agent (A2A) protocol**.

[![Crates.io](https://img.shields.io/crates/v/air-rs.svg)](https://crates.io/crates/air-rs)
[![Spec](https://img.shields.io/badge/spec-draft--1-yellow)](https://agentidentityregistry.org/specs/air/draft-1)

## What is A2A?

A2A is an open protocol for authenticated AI-agent-to-agent messaging. Two agents identified by their `did:wba` DIDs can exchange typed envelopes (`Offer`, `Counter`, `Accept`, `Decline`, `Withdraw`) over signed HTTP. Discovery happens via the W3C did-core `service[]` field in each agent's DID document, resolved through the [AIR](https://agentidentityregistry.org) registry.

## Status

This crate is **pre-v1**. The protocol spec is at `draft-1`. APIs are subject to change until the v1 promotion (target: Phase 3 Week 5 of BossClaw's roadmap).

## Why does this exist?

BossClaw is the reference implementation of trust infrastructure for AI agents. `air-rs` is the protocol code extracted from BossClaw so any other Rust agent (OpenClaw, Mercury, Hermes, future agents) can drop it in via `cargo add air-rs`.

## License

Apache-2.0. Spec text at `/specs/air/draft-1` is CC-BY-4.0.

## Related

- [Agent Identity Registry (AIR)](https://agentidentityregistry.org) — neutral trust scoring + DID document hosting
- [BossClaw](https://github.com/AgentIdentityRegistry/air-note) — desktop forever-companion agent using `air-rs`
- [Spec at draft-1](https://agentidentityregistry.org/specs/air/draft-1) — wire format + conformance vectors
