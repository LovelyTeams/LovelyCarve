// Typed identifiers for the strings the backend keys evidence, filesystem
// nodes and volumes by (`core::common::ids`: `ImageId`, `NodeId`, `VolumeKey`).
//
// They are flavored rather than branded: a plain string (a literal, a stored
// preference, a value read from the DOM) is accepted wherever an id is
// expected, but an id of one kind is rejected where another kind is
// expected, so `fsRead(imageId, nodeId)` cannot receive its arguments swapped.
// Values returned by IPC carry their kind, and so do the parameters of the
// IPC wrappers, the composables and the models that pass them on.

declare const idKind: unique symbol;

type Flavor<Kind extends string> = { readonly [idKind]?: Kind };

/** Loaded evidence: `ImageInfo.id` and the `imageId` of evidence commands. */
export type ImageId = string & Flavor<'ImageId'>;

/** A filesystem node within one evidence: `FsNode.id`. */
export type NodeId = string & Flavor<'NodeId'>;

/** A volume within one evidence (`part:2`, `root`, …), as snapshots key it. */
export type VolumeKey = string & Flavor<'VolumeKey'>;
