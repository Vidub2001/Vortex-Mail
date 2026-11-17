# Vortex Mail — contrôles qualité et sécurité (lot 0 de docs/plan-lots.md).
#
#   make audit            tous les contrôles ; passe au vert sur l'état actuel,
#                         les findings connus étant « acceptés temporairement »
#   make audit-baseline   (re)crée le relevé des avis acceptés de cargo-deny
#   make clippy-strict    Clippy sans tolérance (le même contrôle que make audit)
#   make fmt              vérifie la mise en forme Rust (cargo fmt pour la corriger)
#   make semgrep-test     vérifie les règles Semgrep sur leurs fichiers de test
#   make hooks            Semgrep avant chaque commit (crochet git du dépôt)
#   make sonar            analyse SonarQube (SONAR_HOST_URL et SONAR_TOKEN requis)
#   make fuzz DUREE=60    fuzzing, DUREE secondes par cible (nightly + cargo-fuzz)
#   make ui-test          tests de l'interface dans Chromium (Playwright, lot E4)
#   make ci               make audit pour l'intégration continue (Jenkinsfile, lot G5) :
#                         tests de l'interface obligatoires
#   make audit-tools      vérifie que les outils sont installés
#
# Les rapports sont écrits dans target/audit/ (ignoré par git).

SHELL  := /bin/bash
PYTHON ?= python3
OUT    := target/audit
CLIPPY_JSON := $(OUT)/clippy.json

.PHONY: audit ci ispdb ispdb-age front-test ui-test ui-test-if-ready audit-baseline audit-tools deny npm-audit semgrep semgrep-test \
        fmt clippy clippy-strict panic-unwind test corpus corpus-test sonar fuzz hooks help

help:
	@sed -n '2,18p' Makefile

audit: audit-tools deny npm-audit ispdb-age semgrep fmt clippy panic-unwind test corpus front-test ui-test-if-ready
	@echo
	@echo "✔ make audit : vert (findings connus acceptés temporairement, voir security/)"

# Intégration continue (lot G5) : les mêmes contrôles, mais un Chromium absent
# fait échouer le build au lieu de sauter les tests de l'interface.
ci: audit-tools deny npm-audit ispdb-age semgrep fmt clippy panic-unwind test corpus front-test ui-test
	@echo
	@echo "✔ make ci : vert"

audit-tools:
	@$(PYTHON) scripts/audit/check_tools.py

# Copie de l'ISPDB de Thunderbird (lot I1) : `make ispdb` la rafraîchit (réseau),
# `make audit` refuse une copie de plus de 60 jours.
ispdb:
	$(PYTHON) scripts/ispdb.py maj

ispdb-age:
	@$(PYTHON) scripts/ispdb.py age 60

$(OUT):
	@mkdir -p $(OUT)

# --- Dépendances --------------------------------------------------------------

deny:
	cargo deny check --hide-inclusion-graph advisories bans licenses sources

npm-audit: | $(OUT)
	@npm audit --json > $(OUT)/npm-audit.json || true
	@$(PYTHON) scripts/audit/npm_audit_gate.py $(OUT)/npm-audit.json security/accepted.toml

# --- Règles maison ------------------------------------------------------------

# Bloquant depuis le lot 8 : la moindre occurrence fait échouer, et les règles
# sont d'abord vérifiées sur leurs fichiers de test (une règle cassée ne
# trouverait plus rien). Exceptions : docs/AUDIT.md.
semgrep: semgrep-test | $(OUT)
	@rm -f $(OUT)/semgrep.json
	@semgrep scan --config .semgrep/rules --metrics=off --json --quiet --exclude .semgrep \
		--output $(OUT)/semgrep.json || true
	@$(PYTHON) scripts/audit/semgrep_gate.py $(OUT)/semgrep.json .

semgrep-test:
	@semgrep --test .semgrep/rules

# Crochet git versionné (scripts/hooks/pre-commit) : Semgrep avant chaque commit.
hooks:
	git config core.hooksPath scripts/hooks
	@echo "✔ Semgrep tournera avant chaque commit qui touche du code (git commit --no-verify pour passer outre)"

# --- Clippy -------------------------------------------------------------------

# Bloquant depuis le lot 8 : aucune alerte tolérée. Depuis le lot 10, les
# règles sont dans Cargo.toml ([workspace.lints], d'après BeautifulRust) et
# clippy.toml : cargo clippy seul, l'éditeur et make audit appliquent les mêmes.
# Une exception s'écrit dans le code, avec sa raison :
#   #[expect(clippy::…, reason = "…")]
# (voir docs/AUDIT.md).
CLIPPY_LINTS :=

clippy:
	cargo clippy --workspace --all-targets -- -D warnings $(CLIPPY_LINTS)

clippy-strict: clippy

# Rapport JSON pour SonarQube (make sonar), en avertissements pour tout voir.
$(CLIPPY_JSON): | $(OUT)
	cargo clippy --workspace --all-targets --message-format=json \
		-- $(CLIPPY_LINTS) > $(CLIPPY_JSON)

# --- Mise en forme et déroulement des paniques (lot 10e) ---------------------

# Mise en forme vérifiée, jamais corrigée ici : `cargo fmt` la corrige.
fmt:
	cargo fmt --all -- --check

# mimeparse.rs rattrape une panique de mail-parser (catch_unwind) : ce filet
# disparaîtrait sans bruit avec panic = "abort". On refuse donc ce réglage dans
# les Cargo.toml du dépôt, dans .cargo/config(.toml) et dans RUSTFLAGS.
PANIC_FILES := Cargo.toml src-tauri/Cargo.toml $(wildcard crates/*/Cargo.toml) $(wildcard .cargo/config .cargo/config.toml src-tauri/.cargo/config.toml)

panic-unwind:
	@if grep -nE 'panic[[:space:]]*=[[:space:]]*"abort"|panic=abort' $(PANIC_FILES) \
		|| [[ "$$RUSTFLAGS $$CARGO_ENCODED_RUSTFLAGS" == *panic=abort* ]]; then \
		echo "✘ panic = \"abort\" interdit : il supprimerait le filet de mimeparse.rs (docs/AUDIT.md)"; exit 1; \
	else echo "✔ Paniques déroulées : le filet de mimeparse.rs reste actif"; fi

# --- Tests ---------------------------------------------------------------------

# Tous les tests Rust : unitaires, faux serveur IMAP, droits des fenêtres,
# version de SQLite (relecture du 10 novembre 2026 : make audit ne les lançait pas).
test:
	cargo test --workspace --quiet

# --- Corpus de mails piégés et scénarios de perte de données -------------------

corpus:
	@$(PYTHON) scripts/audit/corpus_check.py tests/corpus
	cargo test -p vortexmail --test corpus --quiet

# Tests du frontend sans navigateur (Node 22.18 ou plus récent).
front-test:
	@npm test --silent

# Tests de l'interface dans Chromium (lot E4, tests/ui/) : version web en mode
# démonstration, comptes fictifs, sans réseau. Installation une fois :
#   npm install && npx playwright install chromium
ui-test:
	npx playwright test

# Dans make audit : lancés si Playwright et Chromium sont installés, signalés sinon.
UI_READY = node --input-type=module -e "import { chromium } from '@playwright/test'; import { existsSync } from 'node:fs'; process.exit(existsSync(chromium.executablePath()) ? 0 : 1)" 2>/dev/null
ui-test-if-ready:
	@if $(UI_READY); then npx playwright test; \
	else echo "⚠ Tests de l'interface non lancés : npx playwright install chromium (voir make ui-test)"; fi

# --- Fuzzing (lot 8, mode d'emploi : fuzz/README.md) ---------------------------

DUREE ?= 60
fuzz:
	scripts/fuzz.sh $(DUREE)

# --- Relevé initial des findings acceptés --------------------------------------

audit-baseline: | $(OUT)
	@$(PYTHON) scripts/audit/deny_baseline.py --clear deny.toml
	@cargo deny --format json check advisories > $(OUT)/deny.json 2>&1 || true
	@$(PYTHON) scripts/audit/deny_baseline.py $(if $(ACCEPTER_NOUVEAUX),--accept-new) $(OUT)/deny.json deny.toml

# --- SonarQube ----------------------------------------------------------------

# URL et jeton viennent de l'environnement, jamais du dépôt. Le scanner les lit
# directement dans SONAR_HOST_URL et SONAR_TOKEN (seuls les noms sont transmis
# à Docker, pas les valeurs).
sonar: | $(OUT)
	@test -n "$$SONAR_HOST_URL" || { echo "SONAR_HOST_URL non défini (ex. export SONAR_HOST_URL=http://sonarqube.exemple.lan:9000)"; exit 1; }
	@test -n "$$SONAR_TOKEN"    || { echo "SONAR_TOKEN non défini (jeton à créer dans SonarQube › Mon compte › Sécurité)"; exit 1; }
	@test -s $(CLIPPY_JSON) || $(MAKE) --no-print-directory $(CLIPPY_JSON)
	@$(PYTHON) scripts/audit/clippy_gate.py sonar $(CLIPPY_JSON) $(OUT)/clippy-sonar.json
	@if command -v sonar-scanner >/dev/null; then \
		sonar-scanner; \
	else \
		docker run --rm --network host -e SONAR_HOST_URL -e SONAR_TOKEN \
			-v "$$PWD:/usr/src" -w /usr/src sonarsource/sonar-scanner-cli; \
	fi
