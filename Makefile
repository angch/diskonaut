.PHONY: build run install test

build:
	cargo build --workspace

run:
	cargo run --bin diskonaut

install:
	cargo install --path diskonaut

test:
	cargo test --workspace
