Walk fixture for the Phantom index pack (SPEC-index G1). Written for this pack.

The test harness copies this tree to a temp directory, renames `gitignore`
to `.gitignore`, and adds a `.git` directory, a file over 1 MiB, and
symlinks. A dotted `.gitignore` here would make the Phantom repo itself
ignore parts of this fixture. The files under `certs/`, `keys/`, and the
`.env*` files hold placeholder text, not secrets.
