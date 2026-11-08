// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::dir::get_dir_entry_inode_by_name;
use crate::inode::Inode;
use crate::{DirEntryName, Ext4, Ext4Error, Path, PathBuf};
use alloc::vec::Vec;

/// How symlinks are treated when looking up an inode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FollowSymlinks {
    /// All symlinks are followed.
    All,

    /// Symlinks are followed, except for the final component. If the
    /// final component is a symlink, the inode for that symlink is
    /// returned rather than the symlink's target.
    ///
    /// This is used for `Ext4::symlink_metadata`, which has similar
    /// behavior to `lstat`:
    /// https://www.man7.org/linux/man-pages/man2/lstat.2.html
    ExcludeFinalComponent,
}

/// Resolve a path to get both the inode it points to and a
/// canonicalized path representation:
///   * Path separators deduplicated ("a//" becomes "a/").
///   * Trailing separators removed ("a/" becomes "a").
///   * "." components removed.
///   * ".." components resolved.
///   * symlink components resolved (except possibly for the last
///     component, depending on `follow`).
///
/// The implementation resolves components one at a time from left to
/// right. Note that absolute symlinks will cause the entire path to be
/// replaced and resolution restarts at the beginning of the path in
/// that case.
///
/// The behavior here should match the way Linux resolves paths. See
/// https://www.man7.org/linux/man-pages/man7/path_resolution.7.html for
/// more details.
///
/// # Errors
///
/// Non-exhaustive list of error conditions:
/// * `path` is not absolute.
/// * `path` does not exist.
/// * Over 40 symlinks are encountered.
/// * Path length ever exceeds 4096 bytes.
///
/// # Panics
///
/// This function panics if path resolution takes over 1000
/// iterations. This should never occur in practice due to other
/// restrictions, this is just a hedge against unforeseen bugs.
pub(crate) fn resolve_path(
    fs: &Ext4,
    path: Path<'_>,
    follow: FollowSymlinks,
) -> Result<(Inode, PathBuf), Ext4Error> {
    // Maximum number of symlinks to resolve (for the whole path, not
    // individual components).
    const MAX_SYMLINKS: usize = 40;
    // Maximum path length in bytes. In general this library does not
    // enforce a path length limit, but during path resolution the
    // length can grow quite a bit due to symlinks.
    const MAX_PATH_LEN: usize = 4096;
    // Maximum number of iterations. This limit should never be reached in
    // practice, this is just to guard against unknown bugs and malicious
    // filesystems that could lead to an infinite loop.
    const MAX_ITERATIONS: usize = 1000;

    if !path.is_absolute() {
        return Err(Ext4Error::NotAbsolute);
    }

    // Check the initial path length. The length will also be checked
    // any time a symlink is spliced into the path, since it might get
    // longer then.
    if path.as_ref().len() > MAX_PATH_LEN {
        return Err(Ext4Error::PathTooLong);
    }

    let mut path = path.as_ref().to_vec();
    // Remove duplicate separators to make the rest of the logic simpler.
    path_dedup_sep(&mut path);

    let mut num_symlinks: usize = 0;
    let mut num_iterations: usize = 0;

    // Current inode, starting at the root.
    let mut inode = fs.read_root_inode()?;

    // Current byte index within the path. Start just after the root `/`.
    let mut index = 1;

    while index < path.len() {
        // OK to unwrap: never exceeds `MAX_ITERATIONS`, which is much
        // less than `usize::MAX`.
        num_iterations = num_iterations.checked_add(1).unwrap();

        // Guard against infinite loops due to bugs or malicious file systems.
        if num_iterations > MAX_ITERATIONS {
            return Err(Ext4Error::PathTooLong);
        }

        // Find the end of the component. This is either the next '/',
        // or the end of the path.
        let next_sep = find_next_sep(&path, index);
        let comp_end = next_sep.unwrap_or(path.len());
        // OK to unwrap: `path` cannot be empty because this function
        // rejects relative paths.
        let last_index = path.len().checked_sub(1).unwrap();
        // This is the last component if there is no next '/', or if the
        // next separator is at the end of the path.
        let is_last_component = next_sep.is_none() || comp_end == last_index;

        // OK to unwrap: `comp_end` is an index in the path, which is
        // limited to `MAX_PATH_LEN`, which is much less than
        // `usize::MAX`.
        let comp_plus_1: usize = comp_end.checked_add(1).unwrap();
        // Index of the separator after the current component, or the
        // end of the path if there isn't a separator.
        let comp_end_with_sep = comp_plus_1.min(path.len());

        // Get the component name.
        let comp = &path[index..comp_end];

        if !inode.metadata.is_dir() {
            // Can't look up a child of a non-directory;
            // path is invalid. This handles a case like
            // "/a/b", where "a" is a regular file instead
            // of a directory.
            return Err(Ext4Error::NotADirectory);
        }

        // Lookup the component's entry in the directory.
        let child_inode =
            get_dir_entry_inode_by_name(fs, &inode, DirEntryName::try_from(comp).unwrap())?;

        if comp == b"." {
            // Remove this component and continue on from the same index.
            path.drain(index..comp_end_with_sep);
        } else if comp == b".." {
            // Remove this component and the previous component (unless
            // this is the first component after the root, in which case
            // the parent is unchanged).
            let remove_start = find_parent_component_start(&path, index);
            path.drain(remove_start..comp_end_with_sep);
            index = remove_start;
            inode = child_inode;
        } else if child_inode.metadata.is_symlink()
            && (follow == FollowSymlinks::All || !is_last_component)
        {
            // Resolve symlink, unless this is the last component and `follow != All`.

            // OK to unwrap: never exceeds `MAX_SYMLINKS`, which is much
            // less than `usize::MAX`.
            num_symlinks = num_symlinks.checked_add(1).unwrap();
            if num_symlinks > MAX_SYMLINKS {
                return Err(Ext4Error::TooManySymlinks);
            }

            let target = child_inode.symlink_target(fs)?;

            let replace_start = if target.is_absolute() {
                // Reset back to the root component.
                inode = fs.read_root_inode()?;
                index = 1;

                // Symlink target is absolute, replace everything up to
                // and including the current component.
                0
            } else {
                // Symlink path is relative. Replace the current
                // component with the target and continue from the
                // current index. Relative symlinks are relative to the
                // directory containing the symlink, so do not change
                // `inode`.
                index
            };

            // Replace the specified range with the symlink's
            // target. The path length may have increased, so check the
            // length limit. Also deduplicate separators again.
            path.splice(replace_start..comp_end, target.as_ref().iter().cloned());
            if path.len() > MAX_PATH_LEN {
                return Err(Ext4Error::PathTooLong);
            }
            path_dedup_sep(&mut path);
        } else {
            // Normal file or directory, or a symlink in the final
            // component in `ExcludeFinalComponent` mode.

            // Continue on to the next component.
            index = comp_end_with_sep;
            inode = child_inode;
        }
    }

    // Handle a separator at the end of the path (unless the path is just '/').
    //
    // If the final component is a directory, remove the trailing
    // separator. Otherwise, it's an error since non-directories don't
    // have children.
    //
    // OK to unwrap: if path is non-empty then `last` is not None.
    if path.len() > 1 && *path.last().unwrap() == Path::SEPARATOR {
        if inode.metadata.is_dir() {
            path.pop();
        } else {
            return Err(Ext4Error::NotADirectory);
        }
    }

    // OK to unwrap: all components of the path have already been validated.
    let output_path = PathBuf::try_from(path).unwrap();

    Ok((inode, output_path))
}

/// Find the index of the next path separator, starting at `start`
/// (inclusive).
///
/// If found, the return value is the absolute index, not an offset from
/// `start`.
///
/// Panics if `start` is not a valid index.
fn find_next_sep(path: &[u8], start: usize) -> Option<usize> {
    assert!(start < path.len());
    (start..path.len()).find(|&i| path[i] == Path::SEPARATOR)
}

/// Find the index of the previous path separator, starting at `start`
/// (inclusive).
///
/// If found, the return value is the absolute index, not an offset from
/// `start`.
///
/// Panics if `start` is not a valid index.
fn find_prev_sep(path: &[u8], start: usize) -> Option<usize> {
    assert!(start < path.len());
    (0..=start).rev().find(|&i| path[i] == Path::SEPARATOR)
}

/// Find the index where the parent component starts.
///
/// `start` is the index of the first byte of a non-root component.
///
/// If this is the first component after the root, then this returns
/// `start` because there is no parent component ("/.." resolves to
/// "/").
///
/// The return value is the absolute index, not an offset from `start`.
///
/// Panics if any of these is true:
/// * `start` is zero.
/// * `start` is not a valid index.
/// * The byte before `start` is not a separator.
fn find_parent_component_start(path: &[u8], start: usize) -> usize {
    assert!(start != 0 && start < path.len());
    assert!(path[start.checked_sub(1).unwrap()] == Path::SEPARATOR);

    if start == 1 {
        // This is the first component after the root, and the
        // parent of root is still the root.
        start
    } else {
        // Find the start of the previous component.

        // Minus 2: minus 1 is the separator before this component,
        // so we want to start searching back from one earlier.
        // OK to unwrap: `start` is at least `2` in this branch.
        let start_search_from = start.checked_sub(2).unwrap();

        // OK to unwrap: this is not the first component after
        // the root, so there must be an earlier separator.
        let prev_sep = find_prev_sep(path, start_search_from).unwrap();

        // Advance to the first byte after the separator.

        // OK to unwrap: `prev_sep` is less than `path.len()`, so the
        // sum still fits in a `usize`.
        prev_sep.checked_add(1).unwrap()
    }
}

/// Replace any duplicate path separators with a single
/// separator. E.g. "a///b" becomes "a/b".
fn path_dedup_sep(path: &mut Vec<u8>) {
    // TODO: would be more efficient to go from end, and to delete in chunks.
    let mut i: usize = 1;
    while i < path.len() {
        // OK to unwrap: `i` is always larger than `1`.
        let prev = i.checked_sub(1).unwrap();
        if path[prev] == Path::SEPARATOR && path[i] == Path::SEPARATOR {
            path.remove(i);
        } else {
            // OK to unwrap: `i` is less than `path.len()`, so the sum
            // still fits in a `usize`.
            i = i.checked_add(1).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_next_sep() {
        assert_eq!(find_next_sep(b"/", 0), Some(0));
        assert_eq!(find_next_sep(b"/abc/", 0), Some(0));
        assert_eq!(find_next_sep(b"/abc/", 1), Some(4));
        assert_eq!(find_next_sep(b"/abc", 1), None);
    }

    #[test]
    fn test_find_prev_sep() {
        assert_eq!(find_prev_sep(b"/", 0), Some(0));
        assert_eq!(find_prev_sep(b"/abc/", 0), Some(0));
        assert_eq!(find_prev_sep(b"/abc/", 1), Some(0));
        assert_eq!(find_prev_sep(b"/abc/", 3), Some(0));
        assert_eq!(find_prev_sep(b"/abc/", 4), Some(4));
        assert_eq!(find_prev_sep(b"abc/", 2), None);
    }

    #[test]
    fn test_find_parent_component_start() {
        assert_eq!(find_parent_component_start(b"/ab/cde/fghi", 1), 1);
        assert_eq!(find_parent_component_start(b"/ab/cde/fghi", 4), 1);
        assert_eq!(find_parent_component_start(b"/ab/cde/fghi", 8), 4);
    }

    #[test]
    fn test_path_dedup_sep() {
        let mut p = b"///a///abc///".to_vec();
        path_dedup_sep(&mut p);
        assert_eq!(p, b"/a/abc/");
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_resolve() {
        let fs = &crate::test_util::load_test_disk1();

        let follow = FollowSymlinks::All;
        let mkp = |s| Path::new(s);

        // Test various things that should all resolve to the root.
        let resolve_to_root = [
            "/",
            "/.",
            "/./",
            "/..",
            "/../",
            "/../..",
            "/../../",
            "/empty_dir/..",
            "/empty_dir/../",
            "/empty_dir/../empty_dir/..",
            "/empty_dir/../empty_dir/../",
            "/dir1/dir2/sym_abs_dir/..",
            "/dir1/dir2/sym_abs_dir/../",
            "/dir1/dir2/sym_rel_dir/..",
            "/dir1/dir2/sym_rel_dir/../",
        ];
        for input in resolve_to_root {
            let (inode, path) = resolve_path(fs, mkp(input), follow).unwrap();
            assert_eq!((inode.index.get(), path.as_path()), (2, mkp("/")));
        }

        // Check directories.
        let (dir_inode, path) = resolve_path(fs, mkp("/dir1/dir2"), follow).unwrap();
        assert_eq!(path, "/dir1/dir2");
        assert!(dir_inode.metadata.is_dir());

        // Check directory with trailing separator.
        let (inode, path) = resolve_path(fs, mkp("/dir1/dir2/"), follow).unwrap();
        assert_eq!(path, "/dir1/dir2");
        assert_eq!(inode.index, dir_inode.index);

        // Check '.' with trailing separator.
        let (inode, path) = resolve_path(fs, mkp("/dir1/dir2/./"), follow).unwrap();
        assert_eq!(path, "/dir1/dir2");
        assert_eq!(inode.index, dir_inode.index);

        let small_file_path = PathBuf::new("/small_file");

        // Check absolute symlink.
        let (inode, path) = resolve_path(fs, mkp("/dir1/dir2/sym_abs"), follow).unwrap();
        assert_eq!(path, small_file_path);
        assert_eq!(fs.read_inode_file(&inode).unwrap(), b"hello, world!");
        let small_file_inode = inode.index;

        // Check relative symlink.
        let (inode, path) = resolve_path(fs, mkp("/dir1/dir2/sym_rel"), follow).unwrap();
        assert_eq!((inode.index, &path), (small_file_inode, &small_file_path));

        // Check absolute symlink followed by additional components.
        let (inode, path) =
            resolve_path(fs, mkp("/dir1/dir2/sym_abs_dir/../small_file"), follow).unwrap();
        assert_eq!((inode.index, &path), (small_file_inode, &small_file_path));

        // Check relative symlink followed by additional components.
        let (inode, path) =
            resolve_path(fs, mkp("/dir1/dir2/sym_rel_dir/../small_file"), follow).unwrap();
        assert_eq!((inode.index, &path), (small_file_inode, &small_file_path));

        // Check that the final symlink is not followed with
        // `ExcludeFinalComponent`.
        let (inode, path) = resolve_path(
            fs,
            mkp("/dir1/dir2/sym_abs"),
            FollowSymlinks::ExcludeFinalComponent,
        )
        .unwrap();
        assert_eq!(path, "/dir1/dir2/sym_abs");
        assert!(inode.metadata.is_symlink());
        let (inode, path) = resolve_path(
            fs,
            mkp("/dir1/dir2/sym_rel"),
            FollowSymlinks::ExcludeFinalComponent,
        )
        .unwrap();
        assert_eq!(path, "/dir1/dir2/sym_rel");
        assert!(inode.metadata.is_symlink());

        // Error: not absolute.
        assert!(matches!(
            resolve_path(fs, mkp("a"), follow),
            Err(Ext4Error::NotAbsolute)
        ));

        // Error: initial path is too long.
        let long_path = "/a".repeat(2049);
        assert!(matches!(
            resolve_path(fs, mkp(&long_path), follow),
            Err(Ext4Error::PathTooLong)
        ));

        // Error: intermediate path is too long. (Same error as above,
        // but difference is visible in code coverage.)
        let long_path = "a/".repeat(2030);
        assert!(matches!(
            resolve_path(fs, mkp("/sym_long").join(long_path).as_path(), follow),
            Err(Ext4Error::PathTooLong)
        ));

        // Error: symlink loop.
        assert!(matches!(
            resolve_path(fs, mkp("/sym_loop_a"), follow),
            Err(Ext4Error::TooManySymlinks)
        ));

        // Error: tried to lookup a child of a regular file.
        assert!(matches!(
            resolve_path(fs, mkp("/empty_file/path"), follow),
            Err(Ext4Error::NotADirectory)
        ));

        // Error: separator after a regular file.
        assert!(matches!(
            resolve_path(fs, mkp("/empty_file/"), follow),
            Err(Ext4Error::NotADirectory)
        ));

        // Error: separator after a trailing component with a symlink in
        // `ExcludeFinalComponent` mode.
        assert!(matches!(
            resolve_path(
                fs,
                mkp("/dir1/dir2/sym_abs_dir/"),
                FollowSymlinks::ExcludeFinalComponent
            ),
            Err(Ext4Error::NotADirectory)
        ));

        // Error: path does not exist.
        assert!(matches!(
            resolve_path(fs, mkp("/empty_dir/does_not_exist"), follow),
            Err(Ext4Error::NotFound)
        ));
    }
}
