#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${SCRIPT_DIR}/logs"
MONITOR_DIR="${SCRIPT_DIR}/monitor"
mkdir -p "$LOG_DIR"

TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
APP_LOG="${LOG_DIR}/roof_control_hub_${TIMESTAMP}.log"
MONITOR_LOG="${LOG_DIR}/monitor_${TIMESTAMP}.log"
TUNNEL_LOG="${LOG_DIR}/tunnel_${TIMESTAMP}.log"

ENABLE_SSH_TUNNEL="${ENABLE_SSH_TUNNEL:-1}"
SSH_REMOTE_HOST="${SSH_REMOTE_HOST:-ronstad.se}"
SSH_REMOTE_USER="${SSH_REMOTE_USER:-olle}"
SSH_REMOTE_PORT="${SSH_REMOTE_PORT:-9091}"
SSH_LOCAL_PORT="${SSH_LOCAL_PORT:-9091}"
SSH_REMOTE_BIND_ADDR="${SSH_REMOTE_BIND_ADDR:-0.0.0.0}"
SSH_TUNNELS="${SSH_TUNNELS:-${SSH_REMOTE_PORT}:${SSH_LOCAL_PORT},9092:9092}"

IFS=',' read -r -a TUNNEL_MAPPINGS <<< "$(echo "$SSH_TUNNELS" | tr -d ' ')"
for mapping in "${TUNNEL_MAPPINGS[@]}"; do
    if ! [[ "$mapping" =~ ^[0-9]+:[0-9]+$ ]]; then
        echo "[$(date)] Invalid SSH_TUNNELS entry '$mapping' (expected remote:local)"
        exit 1
    fi
done

APP_PID=""
MONITOR_STARTED=0
TUNNEL_PID=""

cleanup() {
    echo "[$(date)] Cleaning up..."

    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        echo "[$(date)] Stopping roof control hub (PID $APP_PID)..."
        kill -TERM "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi

    if [ "$MONITOR_STARTED" -eq 1 ] && [ -d "$MONITOR_DIR" ]; then
        echo "[$(date)] Stopping monitor stack..."
        (
            cd "$MONITOR_DIR"
            docker compose down >> "$MONITOR_LOG" 2>&1 || true
        )
    fi

    if [ -n "$TUNNEL_PID" ] && kill -0 "$TUNNEL_PID" 2>/dev/null; then
        echo "[$(date)] Stopping SSH tunnel monitor (PID $TUNNEL_PID)..."
        kill -TERM "$TUNNEL_PID" 2>/dev/null || true
        wait "$TUNNEL_PID" 2>/dev/null || true
    fi

    for mapping in "${TUNNEL_MAPPINGS[@]}"; do
        remote_port="${mapping%%:*}"
        local_port="${mapping##*:}"
        pkill -f "autossh.*${SSH_REMOTE_BIND_ADDR}:${remote_port}:127.0.0.1:${local_port} ${SSH_REMOTE_USER}@${SSH_REMOTE_HOST}" 2>/dev/null || true
    done
}

trap cleanup EXIT SIGINT SIGTERM

echo "Roof Control Hub Startup"
echo "App log: ${APP_LOG}"
echo "Monitor log: ${MONITOR_LOG}"
echo "Tunnel log: ${TUNNEL_LOG}"

cd "$SCRIPT_DIR"

if [ "${START_MONITOR:-1}" = "1" ] && [ -d "$MONITOR_DIR" ]; then
    if command -v docker >/dev/null 2>&1; then
        echo "[$(date)] Starting Prometheus/Grafana stack..."
        (
            cd "$MONITOR_DIR"
            docker compose up -d >> "$MONITOR_LOG" 2>&1
        )
        MONITOR_STARTED=1
    else
        echo "[$(date)] docker not found, skipping monitor startup"
    fi
fi

echo "[$(date)] Building roof control hub..."
cargo build >> "$APP_LOG" 2>&1

echo "[$(date)] Starting roof control hub..."
cargo run >> "$APP_LOG" 2>&1 &
APP_PID=$!
echo "[$(date)] roof control hub started (PID: $APP_PID)"

echo "[$(date)] Waiting for Prometheus endpoint on :9090..."
until curl -fsS http://127.0.0.1:9090/metrics >/dev/null 2>&1; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "[$(date)] ERROR: roof control hub crashed during startup"
        tail -50 "$APP_LOG" || true
        exit 1
    fi
    sleep 2
done

echo "[$(date)] Waiting for controller UI on :9091..."
until curl -fsS http://127.0.0.1:9091/ >/dev/null 2>&1; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "[$(date)] ERROR: roof control hub crashed during startup"
        tail -50 "$APP_LOG" || true
        exit 1
    fi
    sleep 2
done

echo "[$(date)] Roof control hub is ready"
echo "[$(date)] Controller UI: http://localhost:9091"
echo "[$(date)] Prometheus metrics: http://localhost:9090/metrics"

if [ "$MONITOR_STARTED" -eq 1 ]; then
    echo "[$(date)] Prometheus: http://localhost:9092"
    echo "[$(date)] Grafana: http://localhost:3000"
fi

if [ "$ENABLE_SSH_TUNNEL" = "1" ]; then
    if ! command -v autossh >/dev/null 2>&1; then
        echo "[$(date)] autossh not found, skipping SSH tunnel setup"
    else
        echo "[$(date)] Waiting for DNS to be ready for ${SSH_REMOTE_HOST}..."
        until host "$SSH_REMOTE_HOST" >/dev/null 2>&1 || getent hosts "$SSH_REMOTE_HOST" >/dev/null 2>&1; do
            echo "[$(date)] DNS not ready yet"
            sleep 5
        done
        echo "[$(date)] DNS is ready"

        until ip route show default | grep -q .; do
            echo "[$(date)] Waiting for default route"
            sleep 3
        done
        echo "[$(date)] Network route ready"

        echo "[$(date)] Setting up autossh tunnels to ${SSH_REMOTE_HOST} (${SSH_TUNNELS})..."
        while true; do
            if ping -c 1 -w 5 "$SSH_REMOTE_HOST" >/dev/null 2>&1; then
                for mapping in "${TUNNEL_MAPPINGS[@]}"; do
                    remote_port="${mapping%%:*}"
                    local_port="${mapping##*:}"
                    if pgrep -af "autossh.*${SSH_REMOTE_BIND_ADDR}:${remote_port}:127.0.0.1:${local_port} ${SSH_REMOTE_USER}@${SSH_REMOTE_HOST}" >/dev/null 2>&1; then
                        continue
                    fi

                    until ssh -o BatchMode=yes -o ConnectTimeout=5 "${SSH_REMOTE_USER}@${SSH_REMOTE_HOST}" "exit" 2>/dev/null; do
                        echo "[$(date)] Waiting for SSH connectivity to ${SSH_REMOTE_HOST}"
                        sleep 5
                    done

                    echo "[$(date)] Starting autossh tunnel ${SSH_REMOTE_HOST}:${remote_port} -> localhost:${local_port}"
                    autossh -M 0 -fN \
                        -o ServerAliveInterval=30 \
                        -o ServerAliveCountMax=3 \
                        -o ExitOnForwardFailure=yes \
                        -R "${SSH_REMOTE_BIND_ADDR}:${remote_port}:127.0.0.1:${local_port}" "${SSH_REMOTE_USER}@${SSH_REMOTE_HOST}" >> "$TUNNEL_LOG" 2>&1

                    echo "[$(date)] Autossh tunnel established on remote port ${remote_port}"
                done
            else
                echo "[$(date)] Cannot reach ${SSH_REMOTE_HOST}, will retry"
            fi

            sleep 5
        done &
        TUNNEL_PID=$!
        echo "[$(date)] Tunnel monitor started (PID: $TUNNEL_PID)"
        echo "[$(date)] Active tunnel mappings: ${SSH_TUNNELS}"
    fi
fi

wait "$APP_PID"
