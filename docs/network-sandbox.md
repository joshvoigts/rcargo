# Network Sandboxing Limitations

The sandbox allows specifying a domain allowlist (`sandbox.allow.net`
in config), but the actual kernel-level enforcement is coarser:

- **Landlock (Linux) ABI v3**: No network filtering at all. TCP
  bind/connect are unrestricted. The domain list is informational
  only — it has no effect.

- **Landlock ABI v4+ (kernel 5.19+)**: Restricts TCP bind/connect
  by IP address, not by domain name. DNS resolution is unrestricted.
  The domain allowlist is not enforced — we would need to resolve
  domains to IPs before applying the sandbox, which adds complexity.

- **Seatbelt (macOS)**: `(allow network* (remote tcp))` allows all
  TCP connections when the domain list is non-empty. The domain list
  is informational only — Seatbelt does not support domain-based
  filtering.

## Current behavior

The domain list in `sandbox.allow.net` is sent to the shim but has
no effect on actual network restrictions. When the list is
non-empty, network access is fully allowed. When the list is empty,
network access is fully blocked (Landlock ABI v4+, or Seatbelt).

## Follow-up

To properly enforce domain allowlisting:

1. Resolve domains to IPs before entering the sandbox.
2. On Landlock ABI v4+, apply per-IP TCP connect rules.
3. On Seatbelt, consider DNS interception or a network proxy.

This is tracked as future work.
