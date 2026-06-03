# AGENTS.md

We are building a self-hosted Platform as a Service called Billow.

## Layout

This repository is a Rust workspace with the following modules:
- agent - server daemon
- api - gRPC contract between cli and agent
- cli - client CLI
- init - agent installer

Other important files include:
- Justfile
- install.sh - installation script that downloads and triggers the installer
- vm-test.sh - end-to-end smoke test using multipass VMs 

## Useful commands
- Build installation tarball: `just assemble`
- Serve install.sh and tarball: `just serve`
- Build -> Serve -> Run vm-test.sh: `just vm-test`
- standard Rust commands during development

## Practices
- Use `cargo add` / `cargo remove` instead of modifying Cargo.toml directly
- Run `just vm-test` outside the sandbox after every significant code change
 
