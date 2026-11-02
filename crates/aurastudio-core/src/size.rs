//! What a value holds on the heap, for the Memory report and the memory
//! budget: every owner of big data answers it, so a figure in Task
//! Manager can be matched to a cause.

pub trait HeapSize {
    /// Bytes this value owns on the heap (not counting itself).
    fn heap_bytes(&self) -> usize;
}

impl<T: Copy> HeapSize for Vec<T> {
    fn heap_bytes(&self) -> usize { self.capacity() * std::mem::size_of::<T>() }
}

impl<T: Copy> HeapSize for Box<[T]> {
    fn heap_bytes(&self) -> usize { std::mem::size_of_val(&**self) }
}
