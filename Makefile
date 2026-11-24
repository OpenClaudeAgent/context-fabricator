.PHONY: quality qualify-tools bootstrap bootstrap-app format xcode-project \
	package-debug package-release check-swift-analysis check-static test-rust-coverage \
	test-swift-package check-contract test-headless check-product test-native-ui check-native-coverage \
	check-packaged-smoke qualify-runtime-diagnostics check-packaged-corpus check-packaged-outbound \
	check-packaged-telemetry qualify-debug-bundle qualify-release qualify-keychain qualify-github \
	stage-embedding-model qualify-retrieval stage-local-models qualify-answers qualify-github-answers \
	app app-release app-test app-unit app-smoke app-analysis integration verify

quality: qualify-tools

qualify-tools:
	@/bin/bash quality/toolchain/qualify.sh

bootstrap:
	@/bin/bash quality/toolchain/bootstrap.sh

bootstrap-app: bootstrap
	@/bin/bash quality/toolchain/bootstrap-app.sh

xcode-project:
	@/bin/bash quality/macos/app.sh project

package-debug:
	@/bin/bash quality/macos/app.sh build

package-release:
	@/bin/bash quality/macos/app.sh build-release

test-native-ui:
	@/bin/bash quality/macos/app.sh test

NATIVE_UI_REPORTS ?= $(CURDIR)/.quality/ui-shards
check-native-coverage:
	@/bin/bash quality/macos/app.sh merge-ui "$(NATIVE_UI_REPORTS)"

test-swift-package:
	@/bin/bash quality/macos/app.sh unit

check-swift-analysis:
	@/bin/bash quality/macos/analysis.sh

check-static:
	@/bin/bash quality/checks/static.sh
	@$(MAKE) check-swift-analysis

test-rust-coverage:
	@/bin/bash quality/checks/rust-coverage.sh

check-contract:
	@/bin/bash quality/contracts/check.sh

test-headless:
	@/bin/bash -c 'source quality/toolchain/environment.sh && quality_cargo_test "$$quality_root/backend" --locked --workspace'
	@/bin/bash -c 'source quality/toolchain/environment.sh && quality_cargo_test "$$quality_root/cli" --locked --workspace'
	@$(MAKE) test-swift-package

check-product:
	@$(MAKE) check-static
	@$(MAKE) test-rust-coverage
	@$(MAKE) test-swift-package
	@$(MAKE) check-contract

check-packaged-smoke:
	@/bin/bash quality/macos/smoke.sh

qualify-runtime-diagnostics:
	@/bin/bash quality/macos/qualify-lifecycle.sh

check-packaged-corpus:
	@/bin/bash quality/collection/check-corpus.sh

check-packaged-outbound:
	@/bin/bash quality/collection/check-outbound.sh

check-packaged-telemetry:
	@/bin/bash quality/observability/check.sh

qualify-debug-bundle: package-debug
	@$(MAKE) check-packaged-smoke
	@$(MAKE) qualify-runtime-diagnostics
	@$(MAKE) check-packaged-corpus
	@$(MAKE) check-packaged-outbound
	@$(MAKE) check-packaged-telemetry

qualify-keychain: package-debug
	@/bin/bash quality/macos/test-keychain.sh

qualify-github: package-debug
	@/bin/bash quality/github/test-connector.sh

stage-embedding-model:
	@/bin/bash quality/retrieval/stage-model.sh

qualify-retrieval: package-debug
	@/bin/bash quality/retrieval/check.sh

stage-local-models: stage-embedding-model
	@/bin/bash quality/answers/stage-models.sh

qualify-answers: package-debug
	@/bin/bash quality/answers/check.sh

qualify-github-answers: package-debug
	@/bin/bash quality/github/qualify-answers.sh

qualify-release: package-release
	@/bin/bash quality/release/check.sh

verify:
	@$(MAKE) bootstrap-app
	@$(MAKE) qualify-tools
	@$(MAKE) check-product
	@$(MAKE) qualify-debug-bundle
	@$(MAKE) test-native-ui

# Short aliases for local commands.
app: package-debug
app-release: package-release
app-test: test-native-ui
app-unit: test-swift-package
app-smoke: check-packaged-smoke
app-analysis: check-swift-analysis
integration: check-contract

format:
	@cd desktop/macos && xcrun swift-format format --in-place --recursive Package.swift Sources Tests App UITests Client/Package.swift Client/Sources Client/Tests
	@/bin/bash -c 'source quality/toolchain/environment.sh && for workspace in backend cli quality/toolchain/rust-fixture; do cargo fmt --manifest-path "$$workspace/Cargo.toml" --all || exit; done'
