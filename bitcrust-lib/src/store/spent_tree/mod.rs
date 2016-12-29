

/// The spent tree stores the location of transactions in the block-tree
///
/// It is tracks the tree of blocks and is used to verify whether a block can be inserted at a
/// certain location in the tree
///
/// A block consists of the chain of records:
///
/// [block-header] <- [transaction] <- [spent-output] <- [spent-output] <- [transaction] <- ...
///
/// One [spent-output] record is added per input of a transaction.
///
/// Blocks are added regardless of they connect to a previous block. If the previous block comes in
/// later the following blocks are readded.
///
/// Verification takes place on *tip-propagation*. The top pointer is moved forward to the next block
/// after all [spent-outputs] in that block have been checked. This entails scanning back the chain.
/// The scan is succesful if the transaction is found and unsuccesful if it is not found or if the
/// same spent-output is found before the transaction.
///
///
///
///
use std::mem;


use config;

use store::fileptr::FilePtr;
use store::flatfileset::FlatFileSet;

use store::block_content::BlockContent;

pub mod record;
pub use self::record::{Record,RecordPtr};

const MB:                 u32 = 1024 * 1024;
const FILE_SIZE:          u32 = 1024 * MB as u32;
const MAX_CONTENT_SIZE:   u32 = FILE_SIZE - 10 * MB as u32 ;

const SUBPATH: &'static str   = "spent_tree";
const PREFIX:  &'static str   = "st-";

#[derive(Debug, PartialEq)]
pub enum SpendingError {
    OutputNotFound,
    OutputAlreadySpent,
}

pub struct BlockPtr {
    pub start: RecordPtr,
    pub end:   RecordPtr
}


pub struct SpentTree {

    fileset:    FlatFileSet,

}

impl SpentTree {
    pub fn new(cfg: &config::Config) -> SpentTree {

        let dir = &cfg.root.clone().join(SUBPATH);

        SpentTree {
            fileset: FlatFileSet::new(
                dir, PREFIX, FILE_SIZE, MAX_CONTENT_SIZE)
        }
    }


    /// Converts the set of block_content-fileptrs
    /// into a set of records to be stored in the spent_tree
    ///
    pub fn create_block(blockheader: FilePtr, file_ptrs: Vec<FilePtr>) -> Vec<Record> {

        let mut result: Vec<Record> = Vec::with_capacity(file_ptrs.len()+2);

        result.push(Record::new(blockheader.to_block()));

        for ptr in file_ptrs.iter() {

            let mut r = Record::new(*ptr);
            r.set_skip_previous();

            result.push(r);
        };

        let mut rec_end = Record::new(blockheader.to_block());
        rec_end.set_skip_previous();

        result.push(rec_end);


        result
    }


    /// Retrieves the data pointed to by the spent-tree record at `ptr`
    /// This resolves the indirection: The passed ptr points to the spent-tree record
    /// This record points to the block_content
    pub fn load_data_from_spent_tree_ptr<'a>(&'a mut self, block_content: &'a mut BlockContent, ptr: FilePtr) -> &[u8] {
        let rec: &Record = self.fileset.read_fixed(ptr);
        let ptr = rec.ptr;

        block_content.read(ptr)
    }

    /// Stores a block in the spent_tree. The block will be initially orphan.
    ///
    /// The result is a pointer to the first and last record
    pub fn store_block(&mut self, blockheader: FilePtr, file_ptrs: Vec<FilePtr>) -> BlockPtr {

        let block = SpentTree::create_block(blockheader, file_ptrs);


        let result_ptr = self.fileset.write_all(&block);
        let end_ptr = result_ptr.offset(((block.len()-1) * mem::size_of::<Record>()) as i32);

        BlockPtr {
            start: RecordPtr::new(result_ptr),
            end:   RecordPtr::new(end_ptr)
        }
    }

    pub fn find_end(&mut self, target_start: RecordPtr) -> RecordPtr {
        let mut this_ptr = target_start;
        loop {
            this_ptr = this_ptr.next_in_block();

            let record = self.fileset.read_fixed::<Record>(this_ptr.ptr);
            if record.ptr.is_blockheader() {
                return this_ptr;
            }
        }
    }


        /// Verifies of each output in the block at target_start
    pub fn connect_block(&mut self, previous_end: RecordPtr, target_start: RecordPtr) -> Result<RecordPtr, SpendingError> {


        let mut this_ptr = target_start;
        loop {

            this_ptr = this_ptr.next_in_block();

            let record = self.fileset.read_fixed::<Record>(this_ptr.ptr);

            // done?
            if record.ptr.is_blockheader() {

                // we can now make the actual connection
                target_start.set_previous(&mut self.fileset, Some(previous_end));

                return Ok(this_ptr);
            }

            if record.ptr.is_transaction() {
                continue;
            }

            debug_assert!(record.ptr.is_output());

            println!("Testing {:?}", record.ptr);
            // now we scan backwards to see if we find this one
            // both in the current block from this_ptr as in the previous block
            let mut tx_found = false;
            for chain in [this_ptr, previous_end].iter() {

                for prev_rec in chain.iter(&mut self.fileset) {
                    println!("Seek {:?}", prev_rec.ptr);

                    // not the same tx
                    if prev_rec.ptr.file_pos() != record.ptr.file_pos()
                        || prev_rec.ptr.file_number() != record.ptr.file_number() {
                        continue;
                    }

                    if prev_rec.ptr.is_transaction() {
                        tx_found = true;
                        break;
                    }


                    // We have this identical output already spent in the tree?
                    if prev_rec.ptr.is_output()
                        && prev_rec.ptr.output_index() == record.ptr.output_index() {
                        println!("Already spent {:?}", record.ptr);
                        return Err(SpendingError::OutputAlreadySpent);
                    }
                }
                if tx_found {
                    break;
                }
            }
            if !tx_found {
                println!("Not found {:?}", record.ptr);
                return Err(SpendingError::OutputNotFound);
            }

        }

    }


}


#[cfg(test)]
mod tests {

    extern crate tempdir;
    use store::fileptr::FilePtr;


    use config;

    use super::*;

    /// Macro to create a block for the spent_tree tests
    /// blockheaders and txs are unqiue numbers (fileptrs but where they point to doesn't matter
    /// for the spent_tree).
    ///
    /// Construct a block as
    ///
    /// ```
    /// (blk 1 =>                 /* blocknr */
    ///   [tx 2],                /* tx with no inputs  */
    ///   [tx 3 => (2;0),(2;1)]  /* tx with two inputs referencing tx 2 ouput 0 and 1
    /// )
    ///
    macro_rules! block {

        (blk $header:expr =>
          $( [tx $tx:expr  $( => $(   ($tx_in:expr;$tx_in_idx:expr) ),* ),* ] ),*
        )
        =>
        (  ( FilePtr::new(0,$header), vec![
               $( FilePtr::new(0,$tx)  $( ,  $( FilePtr::new(0,$tx_in).to_output($tx_in_idx) ),* ),* ),*
            ])
        )

    }

    impl SpentTree {
        // wrapper around store_block that accepts a tuple instead of two params
        // for easier testing with block! macros
        fn store(&mut self, tuple: (FilePtr, Vec<FilePtr>)) -> BlockPtr {
            self.store_block(tuple.0, tuple.1)
        }
    }

    #[test]
    fn test_spent_tree_connect() {

        let mut st  = SpentTree::new(&config::Config::new_test());

        let block1 = st.store(block!(blk 1 =>
            [tx 2]
        ));

        let block2a = st.store(block!(blk 3 =>
            [tx 4 => (2;0)]
        ));

        let block2b = st.store(block!(blk 5 =>
            [tx 6 => (2;0)]
        ));


        // create a tree, both 2a and 2b attached to 1
        st.connect_block(block1.end, block2a.start).unwrap();
        st.connect_block(block1.end, block2b.start).unwrap();

        // this one should only "fit" onto 2b
        let block3b = st.store(block!(blk 7 =>
            [tx 8 => (6;1)],
            [tx 9 => (2;1)]
        ));


        assert_eq!(
            st.connect_block(block2a.end, block3b.start).unwrap_err(),
            SpendingError::OutputNotFound);

        st.connect_block(block2b.end, block3b.start).unwrap();

        // now this should only fir on 2a and not on 3b as at 3b it is already spent
        let block4a = st.store(block!(blk 10 =>
            [tx 11 => (2;1)],
            [tx 12 => (2;2)]
        ));
        assert_eq!(
            st.connect_block(block3b.end, block4a.start).unwrap_err(),
            SpendingError::OutputAlreadySpent);
        st.connect_block(block2b.end, block4a.start).unwrap();

    }

        #[test]
    fn test_spent_tree() {


        let block1 = block!(blk 1 =>
            [tx 2 => (2;1),(2;0)],
            [tx 3]
        );


        let mut st  = SpentTree::new(& config::Config::new_test() );

        let block_ptr = st.store_block(block1.0, block1.1);

        let block2 = block!(blk 4 =>
            [tx 5 => (2;2),(2;3)],
            [tx 6 ]
        );

        let block_ptr2 = st.store_block(block2.0, block2.1);


        st.connect_block(block_ptr.end, block_ptr2.start).unwrap();

        // this is a bit cumbersome, but we have no accessor function yet so we'll allow this for the
        // test; we want to test the full content of the spent-tree here.
        macro_rules! resolve { ($pt:ident) => (st.fileset.read_fixed::<Record>($pt.ptr).ptr) };

        // we browse backwards and test all values
        let p = block_ptr2.end;
        assert!   (resolve!(p).is_blockheader());
        assert_eq!(resolve!(p).file_pos(), 4);

        let p = { p.prev(&mut st.fileset) };
        assert!(resolve!(p).is_transaction());
        assert_eq!(resolve!(p).file_pos(), 6);

        let p = { p.prev(&mut st.fileset) };
        assert!(resolve!(p).is_output());
        assert_eq!(resolve!(p).file_pos(), 2);
        assert_eq!(resolve!(p).output_index(), 3);

        let p = { p.prev(&mut st.fileset) };
        assert!(resolve!(p).is_output());
        assert_eq!(resolve!(p).file_pos(), 2);
        assert_eq!(resolve!(p).output_index(), 2);

        let p = { p.prev(&mut st.fileset) };
        assert!(resolve!(p).is_transaction());
        assert_eq!(resolve!(p).file_pos(), 5);

        let p = { p.prev(&mut st.fileset) };
        assert!   (resolve!(p).is_blockheader());
        assert_eq!(resolve!(p).file_pos(), 4);


        let p = { p.prev(&mut st.fileset) };
        assert!   (resolve!(p).is_blockheader());
        assert_eq!(resolve!(p).file_pos(), 1);

    }
}

