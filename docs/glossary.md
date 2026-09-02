# Glossary

## Background refresh

A non-blocking task that reads local or remote state and may update a screen
while normal input remains enabled. It uses the refresh task slot.

## Mutation task

A modal operation that changes a repository, extension directory, or shared
Python environment. Input remains blocked until its outcome is known.

## Task outcome

- **Success:** every required stage completed and the resulting state was read.
- **Partial failure:** a mutation happened, but a later stage such as pip or
  state refresh failed.
- **Failure:** the required mutation did not happen. Dependent stages for that
  repository are skipped.

## Repository changed

A notification emitted only after a successful or partially successful
mutation. It invalidates the cached information shown on the main screen.

## Runtime Git configuration

`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_n`, and `GIT_CONFIG_VALUE_n` environment
variables passed only to a spawned Git process. The launcher uses these for URL
mirror rules and `safe.directory=*` without editing global Git configuration.

## Too-small viewport

The isolated rendering state used below 40 columns or 8 rows. Full layouts and
mouse hit-testing resume only after a valid resize and terminal clear.

## Sticky log tail

Log-view behaviour that keeps the newest line visible until the user scrolls
up. Rendering copies only the visible window from the in-memory ring.
