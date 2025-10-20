.PHONY: fmt sql-fmt test coverage docs

RUST_FILES := $(shell find . -name '*.rs' -not -path './target/*')
SQL_FILES := $(shell find . -name '*.sql' -not -path './target/*')

sql-fmt:
	# .sql files
	sleek $(SQL_FILES)
	# inline
	./script/sql-fmt-inline.pl $(RUST_FILES)

fmt: sql-fmt
	cargo fmt

test:
	./script/test.sh

# Simple coverage target
coverage:
	./script/coverage.sh html

docs:
	./script/build-docs.sh
