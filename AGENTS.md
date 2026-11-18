# Branches

- `main` holds the published gpui-pre sources and every patch that mart calls.
- Rewrite no commit of `main` and no commit that mart pinned. Fix a patch in a new commit.
- Branch each patch from `main` as a short-lived `mart/<topic>`, also while mart pins the branch of another session. Branch from that other branch only when the patch needs it, and land that branch first.
- A mart branch may pin the head of its patch branch. Land the patch branch on `main` before mart's `main` pins it, and pin the merge commit there.
- Land a branch by `git merge --no-ff`, so each pinned commit stays reachable. Resolve a conflict with another landed branch in the merge commit. Rebase only a branch that mart never pinned.
- Delete a branch after it lands.
- Tag the head of a dropped branch with commits that no other ref holds as `archive/<branch>`, then delete the branch.
- Drop a patch in a new commit on `main` when mart drops its last caller.
- Describe each patch in the `## Patches` list of `README.md`, in the commit that changes it.

# Upgrade to a later gpui-pre

1. Commit the published sources of the new version, with no changes, on top of the tag `upstream/<current version>`. Tag the commit `upstream/<new version>`.
2. Branch `upgrade/<new version>` from it. Pick each patch of `main` that mart still calls (`git log --no-merges upstream/<current version>..main`), with its later fixes folded in, so each patch is one commit.
3. Drop a commit with a `(cherry picked from commit <sha>)` line when the new version holds that Zed commit.
4. When mart moves to the upgrade branch, run `git merge -s ours main` on it, fast-forward `main` to it and delete the branch.
