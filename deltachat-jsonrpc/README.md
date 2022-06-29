# deltachat-jsonrpc

This crate provides a [JSON-RPC 2.0](https://www.jsonrpc.org/specification) interface to DeltaChat.

The JSON-RPC API is exposed in two fashions:

* The JSON-RPC API can be called through the [C FFI](../deltachat-ffi) with the functions `dc_jsonrpc_init`, `dc_jsonrpc_request`, `dc_jsonrpc_next_response` and `dc_jsonrpc_unref`.
* This crate includes a binary that serves the JSON-RPC API over a [WebSocket](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API).

We also include a JavaScript and TypeScript client for the JSON-RPC API. The source for this is in the [`typescript`](typescript) folder and published on NPM as `deltachat-jsonrpc`.

## Usage

### Running the WebSocket server

From within this folder, you can start the WebSocket server with the following command:

```sh
cargo run --features webserver
```

The server accepts WebSocket connections on `ws://localhost:20808/ws`.

If you are targetting other architectures (like KaiOS or Android), the webserver binary can be cross-compiled easily with [rust-cross](https://github.com/cross-rs/cross):

```sh
cross build --features=webserver --target armv7-linux-androideabi --release
```

### Running the example app

We include a small demo web application that talks to the WebSocket server. To run it, follow these steps:

* The package includes TypeScript bindings which are partially auto-generated through the JSON-RPC library used by this crate ([yerpc](https://github.com/Frando/yerpc/)).
  ```sh
  cd typescript
  npm install
  npm run build
  ```

* Then, build and run the example application:
  ```sh
  npm run example:dev
  ```

## Compiling server for kaiOS or android:

## Run the tests

### Rust tests

```
cargo test --features=webserver
```

### Typescript

```
cd typescript
npm run test
```

For the online tests to run you need a test account token for a mailadm instance,
you can use docker to spin up a local instance: https://github.com/deltachat/docker-mailadm

> set the env var `DCC_NEW_TMP_EMAIL` to your mailadm token: example:
> `DCC_NEW_TMP_EMAIL=https://testrun.org/new_email?t=1h_195dksa6544`

If your test fail with server shutdown at the start, then you might have a process from a last run still running probably and you need to kill that process manually to continue.

#### Test Coverage

You can test coverage with `npm run coverage`, but you need to have `DCC_NEW_TMP_EMAIL` set, otherwise the result will be useless because some functions can only be tested with the online tests.

> If you are offline and want to see the coverage results anyway (even though they are NOT correct), you can bypass the error with `COVERAGE_OFFLINE=1 npm run coverage`

Open `coverage/index.html` for a detailed report.
`bindings.ts` is probably the most interesting file for coverage, because it describes the api functions.