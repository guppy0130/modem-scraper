# hnap-modem-scraper

Built against the Arris Surfboard S33. No promises about other models.

## Errata

* `OTEL_EXPORTER_OTLP_ENDPOINT` should point at the trace endpoint because the
  upstream library passes that value in as-is, instead of checking + appending
  `/v1/traces` as necessary.

```bash
RUST_LOG=debug \
MODEM_SCRAPER_DEVICE_PASSWORD='supersecurepassword' \
MODEM_SCRAPER_TRACE=true \
OTEL_EXPORTER_OTLP_ENDPOINT="http://http.otlp.traces.k3s.home/v1/traces" \
cargo run
```
