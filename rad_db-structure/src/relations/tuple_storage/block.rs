use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::fs::OpenOptions;
use std::hash::Hasher;
use std::io::Write;
use std::io::{BufRead, BufReader, BufWriter};
use std::iter::{FilterMap, Map};
use std::ops::{Deref, DerefMut, Index, IndexMut};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::str::FromStr;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};
use thread::JoinHandle;

use memmap::{Mmap, MmapMut};

use rad_db_types::deserialization::parse_using_types;
use rad_db_types::serialization::serialize_values;
use rad_db_types::Type;

use crate::identifier::Identifier;
use crate::relations::RelationDefinition;
use crate::tuple::Tuple;
use num_bigint::BigUint;
use std::slice::{Iter, IterMut};
use tokio::io::AsyncWrite;

pub struct Block {
    parent_table: Identifier,
    relationship_definition: RelationDefinition,
    block_num: usize,
    block_contents: Option<BlockContents>,
    len: usize,
    usage: RwLock<()>,
    reads: AtomicUsize,
}

impl Block {
    pub fn len(&self) -> usize {
        /*
        let contents = self.get_contents();
        contents.internal.len()

         */
        self.len
    }
}

#[derive(Debug)]
pub struct ReadInUseError;

impl Display for ReadInUseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Could not get readable contents of this block")
    }
}

impl Error for ReadInUseError {}

impl From<PoisonError<RwLockReadGuard<'_, ()>>> for ReadInUseError {
    fn from(_: PoisonError<RwLockReadGuard<'_, ()>>) -> Self {
        ReadInUseError
    }
}

#[derive(Debug)]
pub struct WriteInUseError;

impl Display for WriteInUseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Could not get writable contents of this block")
    }
}

impl Error for WriteInUseError {}

impl From<PoisonError<RwLockWriteGuard<'_, ()>>> for WriteInUseError {
    fn from(_: PoisonError<RwLockWriteGuard<'_, ()>>) -> Self {
        WriteInUseError
    }
}

impl Block {
    pub fn new(
        parent_table: Identifier,
        block_num: usize,
        relationship_definition: RelationDefinition,
    ) -> Self {
        let ret = Block {
            parent_table,
            relationship_definition,
            block_num,
            block_contents: None,
            len: 0,
            usage: RwLock::new(()),
            reads: Default::default(),
        };
        ret.initialize_file().unwrap();
        ret
    }

    fn initialize_file(&self) -> std::io::Result<()> {
        let file_name = self.file_name();

        if file_name.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&file_name.parent().unwrap())?;

        &OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(file_name)?;

        Ok(())
    }

    /// Gets immutable access to the contents of the block
    pub fn get_contents(&self) -> InUse {
        self.try_get_contents().unwrap()
    }

    pub fn try_get_contents(&self) -> Result<InUse, ReadInUseError> {
        let read_guard = self.usage.read()?;
        self.reads.fetch_add(1, Ordering::Acquire);
        if self.block_contents.is_none() {
            unsafe {
                self.load();
            }
        }
        let ret = InUse {
            parent: self,
            read: read_guard,
        };
        Ok(ret)
    }

    /// Attempts to get immutable access to the contents of the block
    pub fn get_contents_mut(&mut self) -> InUseMut {
        self.try_get_contents_mut().unwrap()
    }

    pub fn try_get_contents_mut(&mut self) -> Result<InUseMut, WriteInUseError> {
        let write_copy = (self as *mut Self);
        let write_guard = self.usage.write()?;
        if self.block_contents.is_none() {
            unsafe {
                self.load();
            }
        }
        unsafe {
            let ret = InUseMut {
                parent: &mut *write_copy,
                write: write_guard,
            };
            Ok(ret)
        }
    }

    fn file_name(&self) -> PathBuf {
        let mut ret = PathBuf::from("DB_STORAGE");
        for name in &self.parent_table {
            ret.push(name);
        }
        ret.push(format!("block_{}.txt", self.block_num));
        ret
    }

    /// Determines if the contents of the block is loaded
    fn load_status(&self) -> bool {
        self.block_contents.is_some()
    }

    unsafe fn load(&self) {
        //println!("Loading Block {}", self.block_num);
        let path = self.file_name();
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(&path)
            .expect(&*format!("Could not open file {:?}", path));

        let mut buf_reader = BufReader::new(&file);
        let mut tuples = vec![];
        let mut len = 0;
        loop {
            let mut str = String::new();
            match buf_reader.read_line(&mut str) {
                Err(_) => {
                    panic!("Couldn't read block form file")
                }
                Ok(0) => break,
                Ok(_) => {
                    let str = str.trim_end();
                    let mut split = str.splitn(2, ":");
                    let hash = split.next().unwrap();
                    let tuple_str = split.next().unwrap();

                    let tuple = Tuple::new(
                        parse_using_types(tuple_str, &self.relationship_definition)
                            .expect("Could not parse type")
                            .into_iter(),
                    );
                    len += 1;
                    tuples.push((BigUint::from_str(hash).unwrap(), tuple));
                }
            }
        }

        let contents = BlockContents {
            relationship: self.relationship_definition.clone(),
            file,
            internal: tuples,
        };
        unsafe {
            let mutable = self as *const Self as *mut Self;
            (*mutable).block_contents = Some(contents);
            (*mutable).len = len;
        }
    }

    unsafe fn unload(&self) {
        //println!("Flushing Block {}", self.block_num);
        let unsafe_self = self as *const Self as *mut Self;

        let replaced = std::mem::replace(&mut (*unsafe_self).block_contents, None);
        if let Some(contents) = replaced {
            let BlockContents {
                file: _file,
                internal,
                ..
            } = contents;
            let file_name = self.file_name();
            std::fs::remove_file(&file_name);

            let mut file = File::create(file_name).expect("Failed to recreate file");

            let mut saved = 0;
            let mut buf_writer = BufWriter::new(file);

            for (hash, tuple) in internal {
                writeln!(
                    buf_writer,
                    "{}:{}",
                    hash,
                    serialize_values(tuple.into_iter())
                )
                .unwrap();
                saved += 1;
            }
            //(*unsafe_self).len = saved;
            buf_writer.flush();
            /*
            println!(
                "Saved {} Tuples in {} seconds",
                saved,
                instant.elapsed().as_secs_f64()
            )

             */
        }
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        if self.load_status() {
            unsafe {
                self.unload();
            }
        }
    }
}

pub struct InUse<'a> {
    parent: &'a Block,
    read: RwLockReadGuard<'a, ()>,
}

impl Deref for InUse<'_> {
    type Target = BlockContents;

    fn deref(&self) -> &Self::Target {
        if self.parent.block_contents.is_none() {
            unsafe {
                self.parent.load();
            }
        }
        self.parent.block_contents.as_ref().unwrap()
    }
}

impl Drop for InUse<'_> {
    fn drop(&mut self) {
        if self.parent.reads.fetch_sub(1, Ordering::Acquire) == 1 {
            unsafe { self.parent.unload() }
        }
    }
}

pub struct InUseMut<'a> {
    parent: &'a mut Block,
    write: RwLockWriteGuard<'a, ()>,
}

impl<'a> InUseMut<'a> {
    pub fn insert_tuple(&mut self, hash: BigUint, tuple: Tuple) -> Option<Tuple> {
        let ret = (**self).insert_tuple(hash, tuple);
        if ret.is_none() {
            self.parent.len += 1;
        }
        ret
    }

    pub fn remove_tuple(&mut self, hash: BigUint) -> Option<Tuple> {
        let ret = (**self).remove_tuple(hash);
        if ret.is_some() {
            self.parent.len -= 1;
        }
        ret
    }

    pub fn take_all(&mut self) -> Vec<Tuple> {
        let ret = (**self).take_all();
        self.parent.len = 0;
        ret
    }

    pub fn take_all_with_key(&mut self) -> Vec<(BigUint, Tuple)> {
        let ret = (**self).take_all_with_key();
        self.parent.len = 0;
        ret
    }
}

impl Drop for InUseMut<'_> {
    fn drop(&mut self) {
        unsafe { self.parent.unload() }
    }
}

impl Deref for InUseMut<'_> {
    type Target = BlockContents;

    fn deref(&self) -> &Self::Target {
        self.parent.block_contents.as_ref().unwrap()
    }
}

impl DerefMut for InUseMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.parent.block_contents.as_mut().unwrap()
    }
}

pub struct BlockContents {
    relationship: RelationDefinition,
    file: File,
    internal: Vec<(BigUint, Tuple)>,
}

fn filter_map_helper<T>(input: &Option<T>) -> Option<&T> {
    input.as_ref()
}
fn filter_map_helper_mut<T>(input: &mut Option<T>) -> Option<&mut T> {
    input.as_mut()
}

impl BlockContents {
    pub fn get_tuple(&self, hash: BigUint) -> Option<&Tuple> {
        for (h, tuple) in &self.internal {
            if h == &hash {
                return Some(tuple);
            }
        }
        None
    }

    pub fn get_tuple_mut(&mut self, hash: BigUint) -> Option<&mut Tuple> {
        for (h, tuple) in &mut self.internal {
            if *h == hash {
                return Some(tuple);
            }
        }
        None
    }

    fn insert_tuple(&mut self, hash: BigUint, tuple: Tuple) -> Option<Tuple> {
        if let Some(old) = self.get_tuple_mut(hash.clone()) {
            Some(std::mem::replace(old, tuple))
        } else {
            self.internal.push((hash, tuple));
            None
        }
    }

    fn remove_tuple(&mut self, hash: BigUint) -> Option<Tuple> {
        let pos = self.internal.iter().position(|(t_hash, _)| t_hash == &hash);
        if let Some(pos) = pos {
            Some(self.internal.remove(pos).1)
        } else {
            None
        }
    }

    pub fn get_tuple_from_inner(input: &(BigUint, Tuple)) -> &Tuple {
        &input.1
    }

    pub fn get_tuple_from_inner_mut(input: &mut (BigUint, Tuple)) -> &mut Tuple {
        &mut input.1
    }

    pub fn all(&self) -> Map<Iter<(BigUint, Tuple)>, fn(&(BigUint, Tuple)) -> &Tuple> {
        self.internal.iter().map(Self::get_tuple_from_inner)
    }

    pub fn all_with_key(&self) -> &Vec<(BigUint, Tuple)> {
        &self.internal
    }

    pub fn all_mut(
        &mut self,
    ) -> Map<IterMut<(BigUint, Tuple)>, fn(&mut (BigUint, Tuple)) -> &mut Tuple> {
        self.internal.iter_mut().map(Self::get_tuple_from_inner_mut)
    }

    fn take_all(&mut self) -> Vec<Tuple> {
        let replace = std::mem::replace(&mut self.internal, Vec::new());
        replace.into_iter().map(|(_, t)| t).collect()
    }

    fn take_all_with_key(&mut self) -> Vec<(BigUint, Tuple)> {
        std::mem::replace(&mut self.internal, Vec::new())
    }
}

impl Index<BigUint> for BlockContents {
    type Output = Tuple;

    fn index(&self, index: BigUint) -> &Self::Output {
        self.get_tuple(index).unwrap()
    }
}

impl IndexMut<BigUint> for BlockContents {
    fn index_mut(&mut self, index: BigUint) -> &mut Self::Output {
        self.get_tuple_mut(index).unwrap()
    }
}

impl<'a> IntoIterator for &'a BlockContents {
    type Item = &'a Tuple;
    type IntoIter = Map<Iter<'a, (BigUint, Tuple)>, fn(&(BigUint, Tuple)) -> &Tuple>;

    fn into_iter(self) -> Self::IntoIter {
        self.all()
    }
}

impl<'a> IntoIterator for &'a mut BlockContents {
    type Item = &'a mut Tuple;
    type IntoIter = Map<IterMut<'a, (BigUint, Tuple)>, fn(&mut (BigUint, Tuple)) -> &mut Tuple>;

    fn into_iter(self) -> Self::IntoIter {
        self.all_mut()
    }
}
