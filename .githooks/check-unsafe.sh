#!/bin/bash
# Pre-commit hook: Check that unsafe blocks have SAFETY comments nearby

exit_code=0

for file in "$@"; do
    [ "${file##*.}" = "rs" ] || continue

    # Get all lines containing an `unsafe { ... }` block or an
    # `unsafe impl|fn|trait` declaration. We match the block form anywhere on
    # the line (catches inline expressions like `uid: unsafe { libc::getuid() }`)
    # and the declaration form at the start of a line.
    #
    # The optional visibility prefix accepts `pub`, `pub(crate)`, `pub(super)`,
    # `pub(self)`, and `pub(in path::to::module)` so that a `pub(crate) unsafe
    # fn foo()` is not silently exempt from the SAFETY comment requirement.
    while IFS= read -r line_num; do
        # Check 3 lines before and the current line for SAFETY comment
        start=$((line_num - 3))
        [ $start -lt 1 ] && start=1

        context=$(sed -n "${start},$((line_num))p" "$file")

        if ! echo "$context" | grep -q "SAFETY:"; then
            echo "❌ unsafe without SAFETY comment at $file:$line_num"
            sed -n "$((line_num))p" "$file" | sed 's/^/    /'
            exit_code=1
        fi
    done < <(grep -nE 'unsafe[[:space:]]*\{|^[[:space:]]*(pub([[:space:]]*\([[:space:]]*(crate|super|self|in[[:space:]]+[^)]+)[[:space:]]*\))?[[:space:]]+)?unsafe[[:space:]]+(impl|fn|trait|extern)' "$file" | cut -d: -f1)
done

if [ $exit_code -ne 0 ]; then
    cat << 'EOF'

All 'unsafe' blocks must have a '// SAFETY:' comment explaining why it's safe.
The comment should appear within 3 lines before the unsafe block.

Example:
    // SAFETY: All fields are thread-safe Arc/RwLock types
    unsafe impl Send for MyType {}

Multi-line SAFETY comments:
    // SAFETY: Safe because:
    //  - Field A is always Arc-wrapped
    //  - Field B is protected by RwLock
    unsafe impl Send for MyType {}

EOF
fi

exit $exit_code
