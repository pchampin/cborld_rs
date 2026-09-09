CBOR-LD implementation in Rust
==============================

This project aims to implement the [CBOR-LD] specification as faithfully as possible,
both in order to test the specification itself,
and to provide a compliant implementation to the Rust ecosystem.

It consists of four crates

* [`cborld_lib`](lib) a library implementing [CBOR-LD]
* [`cborld_cli`](cli) a command-line interface to the library above
* [`cborld_test_factory`](test_factory) a library generating CBORD-LD test cases in a systematic way
* [`cborld_test_interop`](test_interop) a test harness used to test and compare multiple implementations of CBOR-LD,
  as well as checking that they are interoperable.

[CBOR-LD]: https://www.w3.org/TR/cbor-ld/
