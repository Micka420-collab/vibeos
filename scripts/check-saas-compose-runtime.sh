#!/usr/bin/env bash
# check-saas-compose-runtime.sh — chaque modèle compose SaaS DÉMARRE vraiment.
#
# POURQUOI
# Le 2026-07-19, le modèle postgres-valkey a été livré alors qu'il crash-loopait au
# premier `up` (postgres:18 refuse l'ancien chemin de volume). check-saas-compose.py
# (statique, loopback-only) ne pouvait pas l'attraper : c'est un comportement au
# DÉMARRAGE. Le bug n'a été trouvé que par un test d'intégration manuel. Ce garde
# le rend automatique : il monte réellement chaque modèle, attend qu'il soit prêt,
# puis le démonte. Il aurait attrapé le bug postgres, et attrapera la régression
# suivante (image qui change de convention, healthcheck cassé, port mal lié…).
#
# ENVIRONNEMENT : exige docker ou podman + compose. Sans runtime → SKIP (rc 0), pour
# ne pas casser les environnements qui n'en ont pas ; il n'est utile (et câblé) que
# dans le workflow saas-compose-integration.yml, où docker est présent.
#
# USAGE : check-saas-compose-runtime.sh [nom-de-modèle]   (défaut : tous)
# Pas de `set -e` : on teste TOUS les modèles et on rapporte, un échec ne coupe pas.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SAAS_DIR="$ROOT/os/rootfs/usr/share/vibeos/saas"
ONLY="${1:-}"

# --- runtime -----------------------------------------------------------------
if docker compose version >/dev/null 2>&1; then
	CE="docker"
elif podman compose version >/dev/null 2>&1; then
	CE="podman"
else
	echo "[SKIP] ni 'docker compose' ni 'podman compose' — test d'intégration non joué."
	exit 0
fi
compose() { "$CE" compose "$@"; }

# --- prépare le répertoire de travail d'un modèle ----------------------------
# Les modèles montent des chemins RELATIFS (./certs, ./prometheus…) : on copie le
# modèle dans un tmp et on y travaille, sans polluer le dépôt.
prepare() {
	local name="$1" work="$2"
	cp -r "$SAAS_DIR/$name/." "$work/"
	# .env : les placeholders des .env.example satisfont déjà les `:?` (dont
	# MEILI_MASTER_KEY >= 16). Insécure mais suffisant pour un test.
	[ -f "$work/.env.example" ] && cp "$work/.env.example" "$work/.env"
	# reverse-proxy : Caddy exige les certs référencés par le Caddyfile.
	if [ "$name" = "reverse-proxy" ]; then
		mkdir -p "$work/certs"
		openssl req -x509 -newkey rsa:2048 -nodes \
			-keyout "$work/certs/local-key.pem" -out "$work/certs/local.pem" \
			-days 1 -subj "/CN=monsaas.localhost" \
			-addext "subjectAltName=DNS:monsaas.localhost" >/dev/null 2>&1
	fi
}

# --- sonde de disponibilité, par modèle --------------------------------------
# rc 0 dès que le modèle est prêt, sinon on boucle jusqu'au timeout.
http_ok() { curl -fsS --max-time 4 -o /dev/null "$1" 2>/dev/null; }
db_healthy() { [ "$("$CE" inspect -f '{{.State.Health.Status}}' "$1" 2>/dev/null)" = "healthy" ]; }
running() { [ "$("$CE" inspect -f '{{.State.Status}}' "$1" 2>/dev/null)" = "running" ]; }

probe() {
	local name="$1" proj="$2"
	case "$name" in
	postgres-valkey) db_healthy "${proj}-db-1" ;;
	observability) http_ok "http://127.0.0.1:9090/-/healthy" && http_ok "http://127.0.0.1:3000/api/health" ;;
	object-storage) http_ok "http://127.0.0.1:8333/status" ;;
	meilisearch) http_ok "http://127.0.0.1:7700/health" ;;
	mailpit) http_ok "http://127.0.0.1:8025/" ;;
	reverse-proxy) running "${proj}-proxy-1" ;; # Caddy chargé (site sur un vhost précis)
	*) running "$(${CE} compose ps -q 2>/dev/null | head -1)" ;;
	esac
}

# --- test d'un modèle --------------------------------------------------------
test_one() {
	local name="$1"
	[ -f "$SAAS_DIR/$name/compose.yaml" ] || {
		echo "   - $name : pas de compose.yaml, ignoré"
		return 0
	}
	local work
	work="$(mktemp -d)"
	local proj="ci-$name"
	# shellcheck disable=SC2064
	trap "cd '$ROOT'; (cd '$work' && ${CE} compose -p '$proj' down -v >/dev/null 2>&1); rm -rf '$work'" RETURN

	prepare "$name" "$work"
	cd "$work" || return 1

	if ! "$CE" compose -p "$proj" up -d >/dev/null 2>&1; then
		echo "   ✗ $name : ÉCHEC du \`compose up\`"
		"$CE" compose -p "$proj" logs --tail 20 2>&1 | sed 's/^/       /'
		return 1
	fi

	local i
	for i in $(seq 1 40); do # 40 x 3s = 2 min max
		if probe "$name" "$proj"; then
			echo "   ✓ $name : prêt (${i}x3s)"
			return 0
		fi
		sleep 3
	done

	echo "   ✗ $name : jamais prêt (timeout 2 min)"
	"$CE" compose -p "$proj" ps -a 2>&1 | sed 's/^/       /'
	"$CE" compose -p "$proj" logs --tail 20 2>&1 | sed 's/^/       /'
	return 1
}

# --- boucle ------------------------------------------------------------------
echo "[..] test d'intégration des modèles compose SaaS (runtime: $CE)"
fails=0
count=0
for dir in "$SAAS_DIR"/*/; do
	name="$(basename "$dir")"
	[ -f "$dir/compose.yaml" ] || continue
	[ -n "$ONLY" ] && [ "$ONLY" != "$name" ] && continue
	count=$((count + 1))
	test_one "$name" || fails=$((fails + 1))
	trap - RETURN
done

if [ "$count" -eq 0 ]; then
	echo "[FAIL] aucun modèle testé (chemin déplacé ?) — fail-closed."
	exit 1
fi
if [ "$fails" -ne 0 ]; then
	echo "[FAIL] $fails/$count modèle(s) compose ne démarrent pas."
	exit 1
fi
echo "[OK] $count modèle(s) compose démarrent et deviennent prêts."
