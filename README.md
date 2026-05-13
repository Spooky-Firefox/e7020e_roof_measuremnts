# Roof Control Hub

Vision pipeline and controller bridge for roof-alignment bring-up on the Rock Pi 4.

This process does two jobs at once:

- waits for a green start circle, then switches the controller to auto once
- estimates roof-grid alignment from a live USB camera feed
- exposes metrics, logs, and a browser UI for the RP2350 controller

The application is headless by default, publishes Prometheus metrics on port `9090`, serves the controller UI on port `9091`, and can optionally forward alignment output over serial.

## Start Here

- [HOW_IT_WORKS.md](HOW_IT_WORKS.md): full pipeline walkthrough, controller bridge, metrics, and tuning notes
- `src/consts.rs`: camera, Hough, confidence, and HTTP bind defaults
- `k8s/` and `argocd/`: Prometheus, Grafana, ingress, and Argo CD sync manifests

## Runtime Surfaces

| Surface | Default | Purpose |
| ------- | ------- | ------- |
| Metrics exporter | `http://127.0.0.1:9090/metrics` | Prometheus scrape endpoint for vision and controller metrics |
| Controller UI | `http://127.0.0.1:9091/` | Browser UI for connect, manual control, mode switching, and telemetry |
| Controller API | `http://127.0.0.1:9091/api/*` | JSON API for ports, connect, commands, telemetry, and log download |
| Prometheus | `https://prometheus.e7012e.ronstad.se` | Remote monitoring endpoint served through Traefik ingress |
| Grafana | `https://e7012e.ronstad.se` | Dashboards for roof alignment and RP2350 telemetry |

The default Grafana dashboard places vehicle/controller panels at the top so controller health and motion state are visible first during bring-up.

## Source Layout

- `src/main.rs`: thread setup, camera init, channel wiring, and process bootstrap
- `src/capture.rs`: USB camera acquisition
- `src/startup.rs`: green-circle startup gate and one-way handoff into roof alignment
- `src/enhance.rs`: Lab-based preprocessing, edges, and dilation
- `src/detect.rs`: Hough-based line detection
- `src/classify.rs`: vertical/horizontal/outlier grouping and statistics
- `src/decide.rs`: alignment estimate and confidence selection
- `src/controller_service.rs`: controller UI, serial bridge, API, and host-side CSV logging
- `src/metrics.rs`: Prometheus exporter for vision and controller data
- `src/controller_ui.html`: embedded browser UI

## Build & Run

### Common commands

| Command | What it does |
| ------- | ------------ |
| `cargo build` | Build the default headless app |
| `cargo run` | Run the headless pipeline with metrics and controller UI |
| `cargo run --no-default-features` | Run with the local OpenCV debug display enabled |
| `./start_server.sh` | Build the app, launch the service, and keep reverse SSH tunnels alive |

### Headless default

```sh
cargo run
```

The default feature set includes `no-display`, which keeps the process suitable for remote operation on the Rock Pi.

At startup, the camera feed stays in a green-circle detection mode. The roof-alignment pipeline does not run until a green circle is confirmed strongly enough to send `mode auto` to the controller. After that handoff, the app latches into normal roof-alignment mode until you reset the startup gate from the UI.

### Local debug display

```sh
cargo run --no-default-features
```

This opens the OpenCV overlay window so you can inspect either the startup green-circle gate or, after handoff, the processed roof-alignment line geometry.

### Full bring-up script

```sh
./start_server.sh
```

The script:

- creates timestamped logs under `logs/`
- waits for `:9090/metrics` and `:9091/` to come up
- optionally creates `autossh` reverse tunnels for controller UI and Prometheus

## Kubernetes + Argo CD

Kubernetes manifests for the monitoring stack + ingress are under `k8s/`, and an Argo CD `Application` is under `argocd/`.

### Files

- `k8s/base/`: namespace, Prometheus, Grafana, vehicle UI external service, and ingress
- `k8s/overlays/prod/`: production overlay and image tag pinning
- `argocd/e7012e-roof-stack-application.yaml`: Argo CD app definition

### Ingress hosts

- `https://e7012e.ronstad.se` -> Grafana
- `https://prometheus.e7012e.ronstad.se` -> Prometheus
- `https://car.ronstad.se` -> controller/car UI

Ingress resources are configured for TLS via Traefik ingress + cert-manager (`letsencrypt-prod` cluster issuer).

### Argo CD setup

1. Update `repoURL` in `argocd/e7012e-roof-stack-application.yaml`.
2. Apply the Argo CD application manifest.

## Environment Variables

| Variable | Default | Purpose |
| -------- | ------- | ------- |
| `ROOF_CAMERA_INDEX` | `4` | Select which camera device to open |
| `ROOF_SERIAL_PORT` | unset | When set, forwards alignment CSV output to that serial port |
| `ENABLE_SSH_TUNNEL` | `1` in `start_server.sh` | Keep a reverse tunnel for the controller UI alive via `autossh` |
| `SSH_REMOTE_HOST` | `ronstad.se` | SSH tunnel destination host |
| `SSH_REMOTE_USER` | `olle` | SSH tunnel destination user |
| `SSH_REMOTE_PORT` | `9091` | Legacy single-tunnel remote bind port used in `SSH_TUNNELS` default |
| `SSH_LOCAL_PORT` | `9091` | Legacy single-tunnel local bind port used in `SSH_TUNNELS` default |
| `SSH_REMOTE_BIND_ADDR` | `0.0.0.0` | Remote bind address for reverse tunnels so cluster ingress can reach forwarded ports |
| `SSH_TUNNELS` | `9091:9091,9092:9090` | Comma-separated `remote:local` reverse tunnel mappings (UI + Prometheus by default) |

## Controller Bridge

The controller service is built into the Rust process and replaces the older standalone Python helper.

### API endpoints

| Method | Path | Purpose |
| ------ | ---- | ------- |
| `GET` | `/api/ports` | List available serial ports |
| `POST` | `/api/connect` | Open the RP2350 serial port and start host-side CSV logging |
| `POST` | `/api/disconnect` | Close the serial link |
| `GET` | `/api/telemetry` | Return the latest parsed controller snapshot |
| `GET` | `/api/startup` | Return the current startup-gate phase and green-circle status |
| `POST` | `/api/command` | Send a raw command string or a batch of command strings to the RP2350 |
| `POST` | `/api/settings` | Update UI-side steering and throttle sensitivity |
| `POST` | `/api/mode` | Send `mode manual` or `mode auto` |
| `POST` | `/api/startup/reset` | Re-arm the startup gate so green-circle detection runs again |
| `GET` | `/api/logs` | List captured controller CSV logs |

### RP2350 commands forwarded by the UI

| Command | Example | Effect |
| ------- | ------- | ------ |
| `pwm-a <microseconds>` | `pwm-a 1600` | Set steering PWM directly in manual mode |
| `pwm-b <microseconds>` | `pwm-b 1525` | Set throttle PWM directly in manual mode |
| `speed <m/s>` | `speed 0.35` | Update the controller speed setpoint |
| `align <angle_rad> <confidence>` | auto-sent by pipeline | Roof angle in radians (6 dp) and confidence (4 dp) |
| `const <name> <value>` | `const observer_q 0.003` | Update a controller runtime tuning constant |
| `mode manual` | `mode manual` | Allow direct PWM commands from the host |
| `mode auto` | `mode auto` | Allow Core 0 to apply control outputs from Core 1 |
| `mode steering auto` | `mode steering auto` | Leave throttle manual while putting steering under automatic control |
| `mode throttle manual` | `mode throttle manual` | Return throttle to manual while leaving steering unchanged |

The browser UI includes a constants tuning panel with:

- a preset const-name dropdown populated with common placeholder names
- a free-text const name field (for any custom name)
- a single-value sender for `const <name> <value>`
- file upload support that sends a batch of const commands in one request

Accepted file line formats are `const <name> <value>` and `<name> <value>`.

The browser UI also shows the startup-gate state:

- current phase (`search_green` or `roof_alignment`)
- current green fill score and EMA
- best detected circle, if any
- a reset button to re-arm the gate without restarting the process

Accepted runtime constant names are `speed`, `setpoint`, `speed_setpoint`, `speed_kp`, `speed_ki`, `speed_kd`, `steering_kp`, `steering_ki`, `steering_kd`, `observer_q`, `observer_r`, and `observer_p0`.

## Telemetry And Logs

- Vision metrics and controller metrics share the same Prometheus exporter.
- Host-side controller CSV logs are written to `controller_logs/`.
- The serial parser accepts both the older 4-column stream and the RP2350 `simple_csv` 14-column event stream.

## Angle Estimate

The reported roof angle is expressed relative to the roof's vertical grid direction, but the estimator does not rely on vertical lines only. It prefers the vertical cluster when enough vertical inliers are present, and falls back to the horizontal cluster when that is the stronger reliable signal for the current corridor section.

## Tuning Notes

The most important vision tuning constants live in `src/consts.rs`:

- `PROCESSING_DOWNSCALE`
- `CANNY_LOW_THRESHOLD`
- `CANNY_HIGH_THRESHOLD`
- `PRIMARY_HOUGH_THRESHOLD`
- `PRIMARY_HOUGH_MIN_LINE_LENGTH`
- `PRIMARY_HOUGH_MAX_LINE_GAP`

If detection gets noisy, raise Hough thresholds before changing the preprocessing stage. If faint roof seams are still missing, confirm the edge image is exposing them before relaxing line filtering.
