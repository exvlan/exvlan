# `@exvlan/runtime`

Shared WebAssembly runtime used by the official Exvlan Browser and
Cloudflare Adapters. Application code should normally install
`@exvlan/browser` or `@exvlan/cloudflare` instead.

The package root contains only cross-platform Exvlan types. The
`@exvlan/runtime/adapter` entry is intended for platform Adapter authors and
hides Guest memory, ABI handles, Host capability operations, and the operation
broker from application-facing Interfaces.

The runtime, Browser Adapter, and Cloudflare Adapter use the same version.
Release automation must publish `@exvlan/runtime` first, followed by
`@exvlan/browser` and `@exvlan/cloudflare`. Platform package manifests use
an exact workspace version, which `pnpm pack` rewrites to an exact registry
dependency. Consumers never run Cargo or an npm install hook.
