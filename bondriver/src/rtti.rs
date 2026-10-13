// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The run-time type information a C++ host expects to find behind the vtable.
//!
//! A host does not simply call through the interface. LibISDB, which is what TVTest and
//! the recorders are built on, does this:
//!
//! ```cpp
//! m_pIBonDriver = pCreateBonDriver();
//! m_pIBonDriver2 = dynamic_cast<IBonDriver2 *>(m_pIBonDriver);
//! ```
//!
//! On MSVC, `dynamic_cast` reads a pointer from **one slot before** the vtable and walks
//! the structures behind it. A table built by hand has nothing there, and the host follows
//! a pointer made of whatever happened to be in memory: measured, TVTest died with
//! 0xC0000409 having called not one method of the interface.
//!
//! So the structures are built here. Their shapes are the ones MSVC emits, and the cast is
//! decided by the **decorated names** below: the host's own `IBonDriver2` lives in its
//! image and ours in this one, so the runtime can only match them by name.
//!
//! On 64-bit the structures refer to each other by image-relative addresses, which cannot
//! be written down at compile time. They are filled in on first use instead, which is also
//! why they have to be static data.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

/// A place for data the loader will fix up, which nothing else touches once
/// [`prepare`] has run.
#[repr(transparent)]
struct Fixed<T>(UnsafeCell<T>);

// Written once, before any host can see it, and read-only afterwards.
unsafe impl<T> Sync for Fixed<T> {}

impl<T> Fixed<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    fn get(&self) -> *mut T {
        self.0.get()
    }
}

/// What a type is called, in the compiler's spelling. `V` marks a class, and
/// the trailing `@@` closes an unqualified name.
type Descriptor = TypeDescriptor<20>;

#[repr(C)]
struct TypeDescriptor<const N: usize> {
    /// Where `type_info`'s own vtable would be. Nothing here uses `typeid`,
    /// and the cast is decided by the name.
    vftable: *const c_void,
    spare: *const c_void,
    name: [u8; N],
}

static IBONDRIVER: Fixed<Descriptor> = Fixed::new(TypeDescriptor {
    vftable: core::ptr::null(),
    spare: core::ptr::null(),
    name: *b".?AVIBonDriver@@\0\0\0\0",
});

static IBONDRIVER2: Fixed<Descriptor> = Fixed::new(TypeDescriptor {
    vftable: core::ptr::null(),
    spare: core::ptr::null(),
    name: *b".?AVIBonDriver2@@\0\0\0",
});

/// Where a base class sits inside the object. All of these sit at the front,
/// so the displacements say "no adjustment".
#[repr(C)]
#[derive(Clone, Copy)]
struct Displacement {
    member: i32,
    pointer: i32,
    virtual_base: i32,
}

const AT_THE_FRONT: Displacement = Displacement {
    member: 0,
    pointer: -1,
    virtual_base: 0,
};

#[repr(C)]
struct BaseClassDescriptor {
    type_descriptor: Reference,
    contained_bases: u32,
    where_it_sits: Displacement,
    attributes: u32,
    class_hierarchy: Reference,
}

#[repr(C)]
struct ClassHierarchyDescriptor {
    signature: u32,
    attributes: u32,
    base_class_count: u32,
    base_class_array: Reference,
}

#[repr(C)]
struct CompleteObjectLocator {
    signature: u32,
    /// Where the vtable this belongs to sits in the object: the front.
    offset: u32,
    constructor_displacement: u32,
    type_descriptor: Reference,
    class_hierarchy: Reference,
    /// Only the 64-bit form has this, and it is how the runtime recovers the
    /// image base: everything else is relative to it.
    #[cfg(target_arch = "x86_64")]
    itself: Reference,
}

/// How one of these structures points at another: an image-relative address on
/// 64-bit, a plain one on 32-bit.
#[cfg(target_arch = "x86_64")]
type Reference = u32;
#[cfg(not(target_arch = "x86_64"))]
type Reference = usize;

static BASE_CLASSES: Fixed<[BaseClassDescriptor; 2]> = Fixed::new([
    // The class itself comes first, and counts the one base below it.
    BaseClassDescriptor {
        type_descriptor: 0,
        contained_bases: 1,
        where_it_sits: AT_THE_FRONT,
        attributes: 0,
        class_hierarchy: 0,
    },
    BaseClassDescriptor {
        type_descriptor: 0,
        contained_bases: 0,
        where_it_sits: AT_THE_FRONT,
        attributes: 0,
        class_hierarchy: 0,
    },
]);

static BASE_CLASS_ARRAY: Fixed<[Reference; 2]> = Fixed::new([0, 0]);

static HIERARCHY: Fixed<ClassHierarchyDescriptor> = Fixed::new(ClassHierarchyDescriptor {
    signature: 0,
    attributes: 0,
    base_class_count: 2,
    base_class_array: 0,
});

static LOCATOR: Fixed<CompleteObjectLocator> = Fixed::new(CompleteObjectLocator {
    // One says the addresses inside are image-relative, which is the 64-bit
    // form; zero says they are plain pointers.
    #[cfg(target_arch = "x86_64")]
    signature: 1,
    #[cfg(not(target_arch = "x86_64"))]
    signature: 0,
    offset: 0,
    constructor_displacement: 0,
    type_descriptor: 0,
    class_hierarchy: 0,
    #[cfg(target_arch = "x86_64")]
    itself: 0,
});

static PREPARED: AtomicBool = AtomicBool::new(false);

/// Fills in how the structures refer to each other, once. Called before the
/// first object is handed out, from the one function a host enters this library
/// through.
pub fn prepare() {
    if PREPARED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let bases = BASE_CLASSES.get();
        (*bases)[0].type_descriptor = reference(IBONDRIVER2.get());
        (*bases)[0].class_hierarchy = reference(HIERARCHY.get());
        (*bases)[1].type_descriptor = reference(IBONDRIVER.get());
        (*bases)[1].class_hierarchy = reference(HIERARCHY.get());

        let array = BASE_CLASS_ARRAY.get();
        (*array)[0] = reference(&raw const (*bases)[0]);
        (*array)[1] = reference(&raw const (*bases)[1]);

        let hierarchy = HIERARCHY.get();
        (*hierarchy).base_class_array = reference(array);

        let locator = LOCATOR.get();
        (*locator).type_descriptor = reference(IBONDRIVER2.get());
        (*locator).class_hierarchy = reference(hierarchy);
        #[cfg(target_arch = "x86_64")]
        {
            (*locator).itself = reference(locator);
        }
    }
}

/// The pointer that belongs one slot before the vtable.
pub fn locator() -> *const c_void {
    LOCATOR.get().cast()
}

/// How one structure refers to another.
#[cfg(target_arch = "x86_64")]
fn reference<T>(address: *const T) -> Reference {
    // Image-relative, and everything referred to is static data of this
    // library, so the subtraction is within the image and fits.
    (address as usize - image_base()) as u32
}

#[cfg(not(target_arch = "x86_64"))]
fn reference<T>(address: *const T) -> Reference {
    address as usize
}

/// Where this library was loaded, which is what the 64-bit form counts from.
#[cfg(target_arch = "x86_64")]
fn image_base() -> usize {
    // The linker puts this at the start of the image, and the loader keeps it
    // there. Asking the loader instead would mean a call and a handle to look
    // after, for the same answer.
    unsafe extern "C" {
        #[link_name = "__ImageBase"]
        static IMAGE_BASE: c_void;
    }
    (&raw const IMAGE_BASE) as usize
}
