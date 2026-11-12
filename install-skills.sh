#!/bin/bash
# Per-skill installs, selected to match the capabilities in README.md.
# Commit the resulting skills-lock.json.
set -e

npx skills add brojonat/llmsrules -s rust -y            # Rust workspace, crates, idioms
npx skills add brojonat/llmsrules -s ibis-data -y       # Python Ibis: golden generation, porting tests
npx skills add obra/superpowers -s test-driven-development -y   # conformance-driven development
npx skills add obra/superpowers -s systematic-debugging -y      # backend semantic diffs
