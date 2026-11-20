.PHONY: quality bootstrap bootstrap-app format app app-test app-unit app-smoke app-analysis integration foundation m02 m03 m04 m05 m06 m07 m08 m08-model m09 m09-models verify

quality:
	@/bin/bash quality/check.sh

bootstrap:
	@/bin/bash quality/bootstrap.sh

bootstrap-app: bootstrap
	@/bin/bash quality/bootstrap-app.sh

app:
	@/bin/bash quality/app.sh build

app-test:
	@/bin/bash quality/app.sh test

app-unit:
	@/bin/bash quality/app.sh unit

app-analysis:
	@/bin/bash quality/app-analysis.sh

app-smoke:
	@/bin/bash quality/integration/smoke-app.sh

integration:
	@/bin/bash quality/integration/check.sh

foundation:
	@/bin/bash quality/foundation.sh

m02: app
	@/bin/bash quality/m02/check.sh

m03: app
	@/bin/bash quality/m03/keychain.sh

m04: app
	@/bin/bash quality/m04/check.sh

m05: app
	@/bin/bash quality/m05/check.sh

m06: app
	@/bin/bash quality/m06/check.sh

m07: app
	@/bin/bash quality/m07/check.sh

m08-model:
	@/bin/bash quality/m08/model.sh

m08: app
	@/bin/bash quality/m08/check.sh

m09-models: m08-model
	@/bin/bash quality/m09/models.sh

m09: app
	@/bin/bash quality/m09/check.sh

verify:
	@$(MAKE) bootstrap-app
	@$(MAKE) quality
	@$(MAKE) foundation
	@$(MAKE) app-test
	@$(MAKE) m02
	@$(MAKE) m04
	@$(MAKE) m05
	@$(MAKE) m06

format:
	@cd desktop/macos && xcrun swift-format format --in-place --recursive Package.swift Sources Tests App UITests Client/Package.swift Client/Sources Client/Tests
	@/bin/bash -c 'source quality/toolchain.sh && for workspace in backend cli quality/m01-rust; do cargo fmt --manifest-path "$$workspace/Cargo.toml" --all || exit; done'
