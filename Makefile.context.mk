# Define the file patterns we're interested in
FILE_PATTERNS := -name "Makefile" -o -name ".gitignore" -o -name "*.md" -o \
                 -name ".looprc" -o -name ".meta.yaml" -o -name "*.rs" -o \
                 -name ".tool-versions" -o -name "*.toml"

# List all contextable files
list-contextable-files:
	@find . \( $(FILE_PATTERNS) \) -type f | sort

# Process .contextignore file if it exists, otherwise return an empty result
process-contextignore:
	@if [ -f .contextignore ]; then \
		sed 's|^/||; s|^#.*||; s|[^/]$$|&/|; s|[^\\]/$$|&*|; /^$$/d' .contextignore; \
	else \
		echo "# .contextignore file not found. No filtering will be applied."; \
	fi

# List filtered contextable-files
list-context-files:
	@sh -c ' \
	contextable_files="$$($(MAKE) -s list-contextable-files)"; \
	ignore_patterns="$$($(MAKE) -s process-contextignore)"; \
	if [ -n "$$ignore_patterns" ] && [ "$$ignore_patterns" != "# .contextignore file not found. No filtering will be applied." ]; then \
		echo "$$ignore_patterns" > .ignore_patterns_tmp; \
		echo "$$contextable_files" | grep -v -f .ignore_patterns_tmp; \
		rm -f .ignore_patterns_tmp; \
	else \
		echo "$$contextable_files"; \
	fi \
	'

# List filtered contextable-files in a tree-like structure
list-context-tree:
	@sh -c ' \
	if command -v tree >/dev/null 2>&1; then \
		$(MAKE) -s list-context-files | tree --fromfile . -L 4 -P "*"; \
	else \
		$(MAKE) -s list-context-files | sed -e "s/[^-][^\/]*\// |/g" -e "s/|\([^ ]\)/|-\1/"; \
	fi \
	'

.PHONY: list-contextable-files process-contextignore list-context-files list-context-tree
