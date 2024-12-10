use std::alloc::{self, Layout};
use std::ops::{DerefMut, Deref};
use std::ptr::NonNull;
use std::ptr;
use std::mem;

#[derive(Debug, Clone)]
pub struct VecTiered<T>  {
    t_size: usize,
    t_count: usize,
    capacity: usize,
    len: usize,
    t_last: usize,
    offsets: Vec<usize>,
    t_swap: NonNull<T>,
    array: NonNull<T>,
}

impl<T> VecTiered<T> where T: Clone {
    pub fn with_capacity(n: usize) -> Self {
        assert!(mem::size_of::<T>() != 0, "We're not ready to handle ZSTs");
        assert!(n > 0, "Cannot allocate empty RotArray");
        let (cap, array_layout, block_size, block_layout) = if n <= 64 {
            (64, Layout::array::<T>(64).unwrap(), 8, Layout::array::<T>(8).unwrap())
        } else {
            // This can't overflow since self.cap <= isize::MAX.
            let k: usize = (f32::sqrt(n as f32) as usize) + 1;
            let cap = k*k;

            // `Layout::array` checks that the number of bytes is <= usize::MAX,
            // but this is redundant since old_layout.size() <= isize::MAX,
            // so the `unwrap` should never fail.
            let array_layout = Layout::array::<T>(cap).unwrap();
            let block_layout = Layout::array::<T>(k).unwrap();
            (cap, array_layout, k, block_layout)
        };

        // Ensure that the new allocation doesn't exceed `isize::MAX` bytes.
        assert!(array_layout.size() <= isize::MAX as usize, "Allocation too large");
        assert!(block_layout.size() <= cap as usize, "Allocation too large");

        let (main_ptr, block_ptr)  = unsafe { (alloc::alloc(array_layout), alloc::alloc(block_layout)) };

        // If allocation fails, `new_ptr` will be null, in which case we abort.
        let ptr = match NonNull::new(main_ptr as *mut T) {
            Some(p) => p,
            None => alloc::handle_alloc_error(array_layout),
        };
        let block = match NonNull::new(block_ptr as *mut T) {
            Some(p) => p,
            None => alloc::handle_alloc_error(block_layout),
        };
        VecTiered {
            array: ptr,
            t_swap: block,
            len: 0,
            capacity: cap,
            t_size: block_size,
            t_count: 1,
            t_last: 0,
            offsets: vec![0; 1],

        }
    }
    pub fn len(&self) -> usize {
        let mut a = 0;
        let mut b = 4;
        mem::swap(&mut a, &mut b);
        self.len
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn clear(&mut self) {
        while let Some(_) = self.pop() { }
    }

    pub fn push(&mut self, elem: T) {
        self.insert(self.len, elem);
    }
    pub fn pop(&mut self) -> Option<T> {
        self.remove(self.len - 1);
        if self.len == 0 {
            return None;
        }
        let index = self.len - 1;
        let s_index = index / self.t_size;
        if self.t_last == self.offsets[s_index] {
            self.t_last = self.t_count - 1;
        } else {
            self.t_last -= 1;
        }
        self.len -= 1;
        let index = self.len;
        unsafe {
            Some(ptr::read(self.array.as_ptr().add(index)))
        }

    }

    pub fn insert(&mut self, index: usize, elem: T) {
        // Note: `<=` because it's valid to insert after everything
        // which is done in push.
        assert!(index <= self.len, "index out of bounds");
        if self.len == self.capacity { self.grow(); }
        let t_index = index / self.t_size;
        if t_index == self.t_last { self.new_t() };
        let sub_index = index % self.t_size;
        let r_index = (sub_index + self.offsets[t_index]) % self.t_size;
        let s_index = (t_index + self.offsets[t_index]) / self.t_size;
        let mut tmp = self.insert_in_slice(s_index, r_index, elem);
        for i in t_index..self.t_count {
            tmp = self.rot_slice_left(i, tmp.clone());
        }
        self.len += 1;
    }

    pub fn remove(&mut self, index: usize) -> T {
        // Note: `<` because it's *not* valid to remove after everything
        assert!(index < self.len, "index out of bounds");
        self.len -= 1;
        unsafe {
            let result = ptr::read(self.array.as_ptr().add(index));
            ptr::copy(
                self.array.as_ptr().add(index + 1),
                self.array.as_ptr().add(index),
                self.len - index,
            );
            result
        }
    }

    fn insert_in_slice(&mut self, t_index: usize, sub_index:usize, elem: T) -> T {
        let l_bound = self.t_size * t_index;
        let mut tmp = elem.clone();
        let mut last = elem.clone();
        self.rebuild_slice(t_index);
        unsafe {
            ptr::swap(&mut last, self.array.as_ptr().add(l_bound + self.t_size - 1));
            ptr::copy(self.array.as_ptr().add(l_bound + sub_index), self.array.as_ptr().add(l_bound + sub_index + 1), self.t_size - sub_index - 1);
            ptr::swap(&mut tmp, self.array.as_ptr().add(l_bound + sub_index));
        }
        return last;
    }

    fn rebuild_slice(&mut self, slice_index: usize) {
        if self.offsets[slice_index] == 0 {
            return;
        }
        let l_bound = self.t_size * slice_index;
        unsafe {
            ptr::copy(
                self.array.as_ptr().add(l_bound),
                self.t_swap.as_ptr(),
                self.offsets[slice_index]
            );
            ptr::copy(
                self.array.as_ptr().add(l_bound + self.offsets[slice_index]),        
                self.array.as_ptr().add(l_bound),
                self.t_size - self.offsets[slice_index]
            );
            ptr::copy(
                self.t_swap.as_ptr(),
                self.array.as_ptr().add(l_bound + self.offsets[slice_index]),
                self.offsets[slice_index]
            );
        }
        self.offsets[slice_index] = 0;
            
    }
    fn rot_slice_left(&mut self, s_index: usize, input: T) -> T {
        let l_bound = self.t_size * s_index;
        let mut tmp = input;
        if self.offsets[s_index] == 0 {
            self.offsets[s_index] = self.t_size - 1;
        } else {
            self.offsets[s_index] -= 1;
        }
        unsafe {
            ptr::swap(&mut tmp, self.array.as_ptr().add(l_bound + self.offsets[s_index]));
        }
        return tmp.to_owned();
    }
    fn rot_slice_right(&mut self, s_index: usize, input: T) -> T {
        let l_bound = self.t_size * s_index;
        let mut tmp = input;
        if self.offsets[s_index] == self.t_size - 1 {
            self.offsets[s_index] = 0;
        } else {
            self.offsets[s_index] += 1;
        }
        unsafe {
            ptr::swap(&mut tmp, self.array.as_ptr().add(l_bound + self.offsets[s_index]));
        }
        return tmp.to_owned();
    }
    fn new_t(&mut self) {
        if self.t_count * self.t_size == self.capacity {
            print!("GROW!"); 
            self.grow();
        }
        self.t_count += 1;
        self.offsets.push(0);
        self.t_last += 1;
    }

    fn grow(&mut self) {
        let (new_cap, new_layout) = if self.capacity < (self.t_size + (self.t_size / 2)).pow(2) {
            let new_cap = self.capacity + (self.t_size * 2);
            (new_cap, Layout::array::<T>(new_cap).unwrap())
        } else {
            panic!("Need to implement tiered grow with rebuild of subarrays");
            // This can't overflow since self.cap <= isize::MAX.
            //let new_cap = 2 * self.cap;

            // `Layout::array` checks that the number of bytes is <= usize::MAX,
            // but this is redundant since old_layout.size() <= isize::MAX,
            // so the `unwrap` should never fail.
            //let new_layout = Layout::array::<T>(new_cap).unwrap();
            //(new_cap, new_layout)
        };

        // Ensure that the new allocation doesn't exceed `isize::MAX` bytes.
        assert!(new_layout.size() <= isize::MAX as usize, "Allocation too large");

        let new_ptr = unsafe { alloc::realloc(self.array.as_ptr() as *mut u8, new_layout, new_cap) };

        // If allocation fails, `new_ptr` will be null, in which case we abort.
        self.array = match NonNull::new(new_ptr as *mut T) {
            Some(p) => p,
            None => alloc::handle_alloc_error(new_layout),
        };
        self.capacity = new_cap;
        self.t_count += 1;
        self.offsets.push(0);
    }

}

impl<T> Deref for VecTiered<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        unsafe {
            std::slice::from_raw_parts(self.array.as_ptr(), self.len)
        }
    }
}
impl<T> DerefMut for VecTiered<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe {
            std::slice::from_raw_parts_mut(self.array.as_ptr(), self.len)
        }
    }
}


impl<T> Drop for VecTiered<T> {
    fn drop(&mut self) {
        if self.capacity != 0 {
            //while let Some(_) = self.pop() { }
            let layout = Layout::array::<T>(self.capacity).unwrap();
            unsafe {
                alloc::dealloc(self.array.as_ptr() as *mut u8, layout);
            }
        }
    }
}


#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {
        let mut v = super::VecTiered::with_capacity(64);
        for i in 0..64 {
            let x = i.clone();
            v.push(x);
        }
        println!("Length {}", v.len());
        assert!(v.len() == 64);
        for i in 0..64 {
            assert!(v.get(i) == Some(&i));
        }

    }
    #[test]
    fn performance_test() {
        let mut v = super::VecTiered::with_capacity(64);
        for i in 0..64 {
            v.push(i);
        }
        for _i in 0..1_000_000 {
            v.insert(32, 100);
            v.remove(32);
        }
    }
}
