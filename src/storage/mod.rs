//! Save data to a desired storage backend.

extern crate ed25519_dalek;
extern crate failure;
extern crate flat_tree as flat;
extern crate random_access_disk as rad;
extern crate random_access_memory as ram;
extern crate random_access_storage as ras;
extern crate sleep_parser;

mod data;
mod node;
mod persist;

pub use self::data::Data;
pub use self::node::Node;
pub use self::persist::Persist;

use self::ed25519_dalek::Signature;
use self::failure::Error;
use self::ras::SyncMethods;
use self::sleep_parser::*;
use std::fmt::Debug;

const HEADER_OFFSET: usize = 32;

/// The types of stores that can be created.
#[derive(Debug)]
pub enum Store {
  /// Tree
  Tree,
  /// Data
  Data,
  /// Bitfield
  Bitfield,
  /// Signatures
  Signatures,
}

/// Result for `Storage.data_offset`
#[derive(Debug)]
pub struct DataOffset {
  length: usize,
  pub offset: usize,
}

impl DataOffset {
  /// Create a new instance.
  #[inline]
  pub fn new(offset: usize, length: usize) -> Self {
    Self { offset, length }
  }
  /// Get the offset.
  #[inline]
  pub fn offset(&self) -> usize {
    self.offset
  }
  /// Get the length.
  #[inline]
  pub fn len(&self) -> usize {
    self.length
  }
  /// Check whether the length is zero.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.length == 0
  }
}

/// Save data to a desired storage backend.
// #[derive(Debug)]
pub struct Storage<T>
where
  T: SyncMethods + Debug,
{
  tree: ras::Sync<T>,
  data: ras::Sync<T>,
  bitfield: ras::Sync<T>,
  signatures: ras::Sync<T>,
}

impl<T> Storage<T>
where
  T: SyncMethods + Debug,
{
  /// Create a new instance. Takes a keypair and a callback to create new
  /// storage instances.
  // Named `.open()` in the JS version. Replaces the `.openKey()` method too by
  // requiring a key pair to be initialized before creating a new instance.
  pub fn new<Cb>(create: Cb) -> Result<Self, Error>
  where
    Cb: Fn(Store) -> ras::Sync<T>,
  {
    let mut instance = Self {
      tree: create(Store::Tree),
      data: create(Store::Data),
      bitfield: create(Store::Bitfield),
      signatures: create(Store::Signatures),
    };

    let header = create_bitfield();
    instance.bitfield.write(0, &header.to_vec())?;

    let header = create_signatures();
    instance.signatures.write(0, &header.to_vec())?;

    let header = create_tree();
    instance.tree.write(0, &header.to_vec())?;

    Ok(instance)
  }

  /// Write data to the feed.
  pub fn write_data(
    &mut self,
    offset: usize,
    data: &[u8],
  ) -> Result<(), Error> {
    self.data.write(offset, &data)
  }

  /// Write a byte vector to a data storage (random-access instance) at the
  /// position of `index`.
  ///
  /// NOTE: Meant to be called from the `.put()` feed method. Probably used to
  /// insert data as-is after receiving it from the network (need to confirm
  /// with mafintosh).
  /// TODO: Ensure the signature size is correct.
  /// NOTE: Should we create a `Data` entry type?
  pub fn put_data(
    &mut self,
    index: usize,
    data: &[u8],
    nodes: &[Node],
  ) -> Result<(), Error> {
    if data.is_empty() {
      return Ok(());
    }

    let offset = self.data_offset(index, nodes)?;

    ensure!(
      offset.len() == data.len(),
      format!("length  `{:?} != {:?}`", offset.len(), data.len())
    );

    self.data.write(offset.offset(), data)
  }

  /// Get data from disk that the user has written to it. This is stored
  /// unencrypted, so there's no decryption needed.
  pub fn get_data(&mut self, index: usize) -> Result<Vec<u8>, Error> {
    let cached_nodes = Vec::new(); // FIXME: reuse allocation.
    let offset = self.data_offset(index, &cached_nodes)?;
    self.data.read(offset.offset(), offset.len())
  }

  /// TODO(yw) docs
  pub fn next_signature(&mut self) {
    unimplemented!();
  }

  /// TODO(yw) docs
  pub fn get_signature(&mut self) {
    unimplemented!();
  }

  /// Write a `Signature` to `self.Signatures`.
  /// TODO: Ensure the signature size is correct.
  /// NOTE: Should we create a `Signature` entry type?
  pub fn put_signature(
    &mut self,
    index: usize,
    signature: Signature,
  ) -> Result<(), Error> {
    self
      .signatures
      .write(HEADER_OFFSET + 64 * index, &signature.to_bytes())
  }

  /// TODO(yw) docs
  /// Get the offset for the data, return `(offset, size)`.
  pub fn data_offset(
    &mut self,
    index: usize,
    cached_nodes: &[Node],
  ) -> Result<DataOffset, Error> {
    let mut roots = Vec::new(); // FIXME: reuse alloc
    flat::full_roots(2 * index, &mut roots);
    let mut offset = 0;
    let mut pending = roots.len();
    let blk = 2 * index;

    for node in cached_nodes {
      println!("root {}", node);
    }

    if pending == 0 {
      let len = match find_node(&cached_nodes, blk) {
        Some(node) => node.len(),
        None => (self.get_node(blk)?).len(),
      };
      println!("len {}", len);
      return Ok(DataOffset::new(offset, len));
    }

    for root in roots {
      // FIXME: we're always having a cache miss here. Check cache first before
      // getting a node from the backend.
      //
      // ```rust
      // let node = match find_node(cached_nodes, root) {
      //   Some(node) => node,
      //   None => self.get_node(root),
      // };
      // ```
      let node = self.get_node(root)?;

      offset += node.len();
      pending -= 1;
      if pending > 0 {
        continue;
      }

      let len = match find_node(&cached_nodes, blk) {
        Some(node) => node.len(),
        None => (self.get_node(blk)?).len(),
      };

      return Ok(DataOffset::new(offset, len));
    }

    panic!("Loop executed without finding max value");
  }

  /// Get a `Node` from the `tree` storage.
  pub fn get_node(&mut self, index: usize) -> Result<Node, Error> {
    let buf = self.tree.read(HEADER_OFFSET + 40 * index, 40)?;
    let node = Node::from_vec(index, &buf)?;
    Ok(node)
  }

  /// TODO(yw) docs
  /// Write a `Node` to the `tree` storage.
  /// TODO: prevent extra allocs here. Implement a method on node that can reuse
  /// a buffer.
  pub fn put_node(&mut self, node: &mut Node) -> Result<(), Error> {
    let index = node.index();
    let buf = node.to_vec()?;
    self.tree.write(HEADER_OFFSET + 40 * index, &buf)
  }

  /// Write data to the internal bitfield module.
  /// TODO: Ensure the chunk size is correct.
  /// NOTE: Should we create a bitfield entry type?
  pub fn put_bitfield(
    &mut self,
    offset: usize,
    data: &[u8],
  ) -> Result<(), Error> {
    self.bitfield.write(HEADER_OFFSET + offset, data)
  }

  /// TODO(yw) docs
  pub fn open_key(&mut self) {
    unimplemented!();
  }
}

/// Get a node from a vector of nodes.
fn find_node(nodes: &[Node], index: usize) -> Option<&Node> {
  for node in nodes {
    if node.index() == index {
      return Some(node);
    }
  }
  None
}
