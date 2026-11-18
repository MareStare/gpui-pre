# Branches

- `main` holds the published gpui-pre sources and every patch that mart calls. Mart pins a commit of `main`.
- Rewrite no commit of `main` and no commit that mart pinned. Fix a patch in a new commit.
- Land each patch on `main` from a short-lived `mart/<topic>` branch off `main`, as a fast-forward after a rebase. Delete the branch after it lands.
- Tag the head of a dropped branch with commits that no other ref holds as `archive/<branch>`, then delete the branch.
- Drop a patch in a new commit on `main` when mart drops its last caller.
- Describe each patch in the `## Patches` list of `README.md`, in the commit that changes it.

# Upgrade to a later gpui-pre

1. Commit the published sources of the new version, with no changes, on top of the tag `upstream/<current version>`. Tag the commit `upstream/<new version>`.
2. Branch `upgrade/<new version>` from it. Pick each patch of `main` that mart still calls (`git log --no-merges upstream/<current version>..main`), with its later fixes folded in, so each patch is one commit.
3. Drop a commit with a `(cherry picked from commit <sha>)` line when the new version holds that Zed commit.
4. When mart moves to the upgrade branch, run `git merge -s ours main` on it, fast-forward `main` to it and delete the branch.
