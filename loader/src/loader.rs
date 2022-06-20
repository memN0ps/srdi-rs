use std::{arch::asm, ffi::{CStr}, collections::BTreeMap, mem::size_of};

use winapi::{um::{winnt::{PIMAGE_DOS_HEADER, IMAGE_DIRECTORY_ENTRY_EXPORT, PIMAGE_EXPORT_DIRECTORY, PIMAGE_SECTION_HEADER, IMAGE_DIRECTORY_ENTRY_IMPORT, PIMAGE_IMPORT_DESCRIPTOR, PIMAGE_IMPORT_BY_NAME, IMAGE_IMPORT_DESCRIPTOR, PIMAGE_BASE_RELOCATION, IMAGE_DIRECTORY_ENTRY_BASERELOC, IMAGE_BASE_RELOCATION, IMAGE_REL_BASED_DIR64, MEM_RESERVE, MEM_COMMIT, PAGE_EXECUTE_READWRITE, DLL_PROCESS_ATTACH, IMAGE_REL_BASED_HIGHLOW}}, shared::{minwindef::{HMODULE, FARPROC, LPVOID, DWORD, HINSTANCE, BOOL}, ntdef::{LPCSTR, HANDLE, PVOID, NTSTATUS}, basetsd::SIZE_T}, ctypes::c_void};
use ntapi::{ntpebteb::PTEB, ntldr::{PLDR_DATA_TABLE_ENTRY}, ntpsapi::PEB_LDR_DATA};
use wchar::wch;

#[cfg(target_arch = "x86")]
use winapi::{um::winnt::{PIMAGE_NT_HEADERS32, PIMAGE_THUNK_DATA32, IMAGE_SNAP_BY_ORDINAL32, IMAGE_ORDINAL32}};

#[cfg(target_arch = "x86_64")]
use winapi::{um::winnt::{PIMAGE_NT_HEADERS64, PIMAGE_THUNK_DATA64, IMAGE_SNAP_BY_ORDINAL64, IMAGE_ORDINAL64}};


#[allow(non_camel_case_types)]
type fnLoadLibraryA = unsafe extern "system" fn(lpFileName: LPCSTR) -> HMODULE;

#[allow(non_camel_case_types)]
type fnGetProcAddress = unsafe extern "system" fn(
    hModule: HMODULE, 
    lpProcName: LPCSTR
) -> FARPROC;

#[allow(non_camel_case_types)]
type fnNtFlushInstructionCache = unsafe extern "system" fn(
    ProcessHandle: HANDLE, 
    BaseAddress: PVOID, 
    Length: SIZE_T
) -> NTSTATUS;


#[allow(non_camel_case_types)]
type fnVirtualAlloc = unsafe extern "system" fn(
    lpAddress: LPVOID, 
    dwSize: SIZE_T, 
    flAllocationType: DWORD, 
    flProtect: DWORD
) -> LPVOID;

#[allow(non_camel_case_types)]
type fnDllMain = unsafe extern "system" fn(
    module: HINSTANCE,
    call_reason: DWORD,
    reserved: LPVOID,
) -> BOOL;


// Hash
const KERNEL32DLL_HASH: u32 = fnv1a_hash_32_wstr(wch!("kernel32.dll"));
const NTDLLDLL_HASH: u32 = fnv1a_hash_32_wstr(wch!("ntdll.dll"));

const LOADLIBRARYA_HASH: u32 = fnv1a_hash_32("LoadLibraryA".as_bytes());
const GETPROCADDRESS_HASH: u32 = fnv1a_hash_32("GetProcAddress".as_bytes());
const VIRTUALALLOC_HASH: u32 = fnv1a_hash_32("VirtualAlloc".as_bytes());
const NTFLUSHINSTRUCTIONCACHE_HASH: u32 = fnv1a_hash_32("NtFlushInstructionCache".as_bytes());

// Function pointers (Thanks B3NNY)
static mut LOAD_LIBRARY_A: Option<fnLoadLibraryA> = None;
static mut GET_PROC_ADDRESS: Option<fnGetProcAddress> = None;
static mut VIRTUAL_ALLOC: Option<fnVirtualAlloc> = None;
static mut NT_FLUSH_INSTRUCTION_CACHE: Option<fnNtFlushInstructionCache> = None;


fn get_peb_ldr() -> usize {
    let teb: PTEB;
	unsafe {
        #[cfg(target_arch = "x86")]
		asm!("mov {teb}, fs:[0x18]", teb = out(reg) teb);

		#[cfg(target_arch = "x86_64")]
		asm!("mov {teb}, gs:[0x30]", teb = out(reg) teb);
	}

	let teb = unsafe { &mut *teb };
	let peb = unsafe { &mut *teb.ProcessEnvironmentBlock };
	let peb_ldr = peb.Ldr;

    peb_ldr as _
}

pub fn set_exported_functions_by_hash(peb_ldr: *mut PEB_LDR_DATA) {
    // get kernel32 base address via hash
    let kernel32_base = unsafe { get_loaded_module_by_hash(peb_ldr, KERNEL32DLL_HASH).expect("failed to kernel32 by hash") };
    log::info!("[+] KERNEL32: {:?}", kernel32_base);
    
    // get ntdll base address via hash
    let ntdll_base = unsafe { get_loaded_module_by_hash(peb_ldr, NTDLLDLL_HASH).expect("failed to get ntdll by hash") };
    log::info!("[+] NTDLL: {:?}", ntdll_base);

    // get exports by hash and store the their virtual address
    //kernel32
    let loadlibrarya_address = get_exports_by_hash(kernel32_base, LOADLIBRARYA_HASH).expect("failed to get LoadLibraryA by hash");
    unsafe { LOAD_LIBRARY_A = Some(std::mem::transmute::<_, fnLoadLibraryA>(loadlibrarya_address)) };
    log::info!("[+] LoadLibraryA {:?}", loadlibrarya_address);

    let getprocaddress_address = get_exports_by_hash(kernel32_base, GETPROCADDRESS_HASH).expect("failed to get GetProcAddress by hash");
    unsafe { GET_PROC_ADDRESS = Some(std::mem::transmute::<_, fnGetProcAddress>(getprocaddress_address)) };
    log::info!("[+] GetProcAddress {:?}", getprocaddress_address);

    let virtualalloc_address = get_exports_by_hash(kernel32_base, VIRTUALALLOC_HASH).expect("failed to get VirtualAlloc by hash");
    unsafe { VIRTUAL_ALLOC = Some(std::mem::transmute::<_, fnVirtualAlloc>(virtualalloc_address)) };
    log::info!("[+] VirtualAlloc {:?}", virtualalloc_address);

    //ntdll
    let ntflushinstructioncache_address = get_exports_by_hash(ntdll_base, NTFLUSHINSTRUCTIONCACHE_HASH).expect("failed to get NtFlushInstructionCache by hash");
    unsafe { NT_FLUSH_INSTRUCTION_CACHE = Some(std::mem::transmute::<_, fnNtFlushInstructionCache>(ntflushinstructioncache_address)) };
    log::info!("[+] NtFlushInstructionCache {:?}", ntflushinstructioncache_address);
}


pub fn reflective_loader(dll_bytes: &'static [u8]) {

    let module_base = dll_bytes.as_ptr() as usize;

    let dos_header = module_base as PIMAGE_DOS_HEADER;
    log::info!("[+] Image_DOS_HEADER: {:?}", module_base);

    #[cfg(target_arch = "x86")]
    let nt_headers = unsafe { (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32 };
    #[cfg(target_arch = "x86_64")]
    let nt_headers = unsafe { (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64 };
    log::info!("[+] IMAGE_NT_HEADERS: {:?}", nt_headers);

    let peb_ldr = get_peb_ldr() as *mut PEB_LDR_DATA;
    log::info!("[*] PEB_LDR_DATA {:?}", peb_ldr);

    //LOAD_LIBRARY_A, GET_PROC_ADDRESS, VIRTUAL_ALLOC, NT_FLUSH_INSTRUCTION_CACHE
    set_exported_functions_by_hash(peb_ldr);
    
    let new_module_base = unsafe { copy_sections_to_local_process(module_base) };
    log::info!("[+] New Module Base: {:?}", new_module_base);
    
    // STEP 3: process all of our images relocations...
    log::info!("[*] Rebasing Image");
    unsafe { rebase_image(module_base as _, new_module_base) };

    // STEP 4: process our images import table...
    log::info!("[*] Resolving Imports");
    unsafe { resolve_imports(new_module_base) };


    // STEP 5: call our images entry point
    let entry_point = unsafe { new_module_base as usize + (*nt_headers).OptionalHeader.AddressOfEntryPoint as usize };
    log::info!("allocated_image_base {:?} + AddressOfEntryPoint {:#x} = {:#x}", new_module_base, unsafe { (*nt_headers).OptionalHeader.AddressOfEntryPoint }, entry_point);

    // We must flush the instruction cache to avoid stale code being used which was updated by our relocation processing.
    unsafe { NT_FLUSH_INSTRUCTION_CACHE.unwrap()(-1 as _, std::ptr::null_mut(), 0) };

    log::info!("Calling DllMain");
    
    #[allow(non_snake_case)]
    let DllMain = unsafe { std::mem::transmute::<_, fnDllMain>(entry_point) };

    pause();

    // STEP 6: The DLLMain function to be executed
    unsafe { DllMain(new_module_base as _, DLL_PROCESS_ATTACH, std::ptr::null_mut()) };

    // STEP 7: return our new entry point address so whatever called us can call DllMain() if needed.
    //return;
}



/// Rebase the image / perform image base relocation
unsafe fn rebase_image(module_base: *mut u8, new_module_base: *mut c_void) {

    let dos_header = module_base as PIMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64;

    // Calculate the difference between remote allocated memory region where the image will be loaded and preferred ImageBase (delta)
    let delta = new_module_base as isize - (*nt_headers).OptionalHeader.ImageBase as isize;
    log::info!("[+] Allocated Memory: {:?} - Current ImageBase: {:#x} = Delta: {:#x}", new_module_base, (*nt_headers).OptionalHeader.ImageBase, delta);


    // Calcuate the dos/nt headers of new_module_base
    // Resolve the imports of the newly allocated memory region 

    let dos_header = new_module_base as PIMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers = (new_module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers = (new_module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64;

    // Get a pointer to the first _IMAGE_BASE_RELOCATION
    let mut base_relocation = (new_module_base as usize 
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC as usize].VirtualAddress as usize) as PIMAGE_BASE_RELOCATION;
    
    // Get the end of _IMAGE_BASE_RELOCATION
    let base_relocation_end = base_relocation as usize 
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC as usize].Size as usize;
    

    while (*base_relocation).VirtualAddress != 0u32 && (*base_relocation).VirtualAddress as usize <= base_relocation_end && (*base_relocation).SizeOfBlock != 0u32 {
        
        // Get the VirtualAddress, SizeOfBlock and entries count of the current _IMAGE_BASE_RELOCATION block
        let address = (new_module_base as usize + (*base_relocation).VirtualAddress as usize) as isize;
        let item = (base_relocation as usize + std::mem::size_of::<IMAGE_BASE_RELOCATION>()) as *const u16;
        let count = ((*base_relocation).SizeOfBlock as usize - std::mem::size_of::<IMAGE_BASE_RELOCATION>()) / std::mem::size_of::<u16>() as usize;

        for i in 0..count {
            // Get the Type and Offset from the Block Size field of the _IMAGE_BASE_RELOCATION block
            let type_field = item.offset(i as isize).read() >> 12;
            let offset = item.offset(i as isize).read() & 0xFFF;

            //IMAGE_REL_BASED_DIR32 does not exist
            //#define IMAGE_REL_BASED_DIR64   10
            if type_field == IMAGE_REL_BASED_DIR64 || type_field == IMAGE_REL_BASED_HIGHLOW {
                // Add the delta to the value of each address where the relocation needs to be performed
                *((address + offset as isize) as *mut isize) += delta;
            }
        }

        // Get a pointer to the next _IMAGE_BASE_RELOCATION
        base_relocation = (base_relocation as usize + (*base_relocation).SizeOfBlock as usize) as PIMAGE_BASE_RELOCATION;
    }
}

/// Resolve the image imports
unsafe fn resolve_imports(new_module_base: *mut c_void) {
    let dos_header = new_module_base as PIMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers = (new_module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers = (new_module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64;


    // Get a pointer to the first _IMAGE_IMPORT_DESCRIPTOR
    let mut import_directory = (new_module_base as usize 
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT as usize].VirtualAddress as usize) as PIMAGE_IMPORT_DESCRIPTOR;

    while (*import_directory).Name != 0 {

        // Get the name of the dll in the current _IMAGE_IMPORT_DESCRIPTOR
        let dll_name = (new_module_base as usize 
            + (*import_directory).Name as usize) as *const i8;        
        
            // Load the DLL in the in the address space of the process
        let dll_handle = LOAD_LIBRARY_A.unwrap()(dll_name); //call function pointer LOAD_LIBRARY_A

        // Get a pointer to the OriginalFirstThunk in the current _IMAGE_IMPORT_DESCRIPTOR
        #[cfg(target_arch = "x86")]
        let mut original_first_thunk = (new_module_base as usize 
            + *(*import_directory).u.OriginalFirstThunk() as usize) as PIMAGE_THUNK_DATA32;

        // Get a pointer to the OriginalFirstThunk in the current _IMAGE_IMPORT_DESCRIPTOR
        #[cfg(target_arch = "x86_64")]
        let mut original_first_thunk = (new_module_base as usize 
            + *(*import_directory).u.OriginalFirstThunk() as usize) as PIMAGE_THUNK_DATA64;

        // Get a pointer to the FirstThunk in the current _IMAGE_IMPORT_DESCRIPTOR
        #[cfg(target_arch = "x86")]
        let mut thunk = (new_module_base as usize 
            + (*import_directory).FirstThunk as usize) 
            as PIMAGE_THUNK_DATA32;
        
        // Get a pointer to the FirstThunk in the current _IMAGE_IMPORT_DESCRIPTOR
        #[cfg(target_arch = "x86_64")]
        let mut thunk = (new_module_base as usize 
            + (*import_directory).FirstThunk as usize) 
            as PIMAGE_THUNK_DATA64;
 
        while (*original_first_thunk).u1.Function() != &0 {
            
            // Get a pointer to _IMAGE_IMPORT_BY_NAME
            let thunk_data = (new_module_base as usize
                + *(*original_first_thunk).u1.AddressOfData() as usize)
                as PIMAGE_IMPORT_BY_NAME;

            #[cfg(target_arch = "x86")]
            // #define IMAGE_SNAP_BY_ORDINAL32(Ordinal) ((Ordinal & IMAGE_ORDINAL_FLAG32) != 0)
            let result = IMAGE_SNAP_BY_ORDINAL32(*(*original_first_thunk).u1.Ordinal());

            // #define IMAGE_SNAP_BY_ORDINAL64(Ordinal) ((Ordinal & IMAGE_ORDINAL_FLAG64) != 0)
            #[cfg(target_arch = "x86_64")]
            let result = IMAGE_SNAP_BY_ORDINAL64(*(*original_first_thunk).u1.Ordinal());

            if result {
                //#define IMAGE_ORDINAL32(Ordinal) (Ordinal & 0xffff)
                #[cfg(target_arch = "x86")]
                let fn_ordinal = IMAGE_ORDINAL32(*(*original_first_thunk).u1.Ordinal()) as _;

                //#define IMAGE_ORDINAL64(Ordinal) (Ordinal & 0xffff)
                #[cfg(target_arch = "x86_64")]
                let fn_ordinal = IMAGE_ORDINAL64(*(*original_first_thunk).u1.Ordinal()) as _;

                *(*thunk).u1.Function_mut() = GET_PROC_ADDRESS.unwrap()(dll_handle, fn_ordinal) as _; // call function pointer GET_PROC_ADDRESS
            } else {
                // Get a pointer to the function name in the IMAGE_IMPORT_BY_NAME
                let fn_name = (*thunk_data).Name.as_ptr();
                // Retrieve the address of the exported function from the DLL and ovewrite the value of "Function" in the IMAGE_THUNK_DATA64
                *(*thunk).u1.Function_mut() = GET_PROC_ADDRESS.unwrap()(dll_handle, fn_name) as _; // call function pointer GET_PROC_ADDRESS
            }

            // Increment Thunk and OriginalFirstThunk
            thunk = thunk.offset(1);
            original_first_thunk = original_first_thunk.offset(1);
        }

        // Get a pointer to the next _IMAGE_IMPORT_DESCRIPTOR
        import_directory = (import_directory as usize + size_of::<IMAGE_IMPORT_DESCRIPTOR>() as usize) as _;
    }
}

// Copy sections of the dll to a memory location
unsafe fn copy_sections_to_local_process(module_base: usize) -> *mut c_void { //Vec<u8>
    
    let dos_header = module_base as PIMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64;

    let image_size = (*nt_headers).OptionalHeader.SizeOfImage as usize;
    let image_base = (*nt_headers).OptionalHeader.ImageBase as *mut c_void;

    //Heap or VirtualAlloc (RWX or RW and later X)
    //let mut image = vec![0; image_size];
    let mut image = VIRTUAL_ALLOC.unwrap()(image_base, image_size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);

    if image.is_null() {
        image = VIRTUAL_ALLOC.unwrap()(std::ptr::null_mut(), image_size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    }

    // get a pointer to the _IMAGE_SECTION_HEADER
    let section_header = (&(*nt_headers).OptionalHeader as *const _ as usize + (*nt_headers).FileHeader.SizeOfOptionalHeader as usize) as PIMAGE_SECTION_HEADER;

    for i in 0..(*nt_headers).FileHeader.NumberOfSections {
        // get a reference to the current _IMAGE_SECTION_HEADER
        let section_header_i = &*(section_header.add(i as usize));

        // get the pointer to current section header's virtual address
        //let destination = image.as_mut_ptr().add(section_header_i.VirtualAddress as usize);
        let destination = image.cast::<u8>().add(section_header_i.VirtualAddress as usize);
        log::info!("destination: {:?}", destination);
        
        // get a pointer to the current section header's data
        let source = image_base as usize + section_header_i.PointerToRawData as usize;
        log::info!("source: {:#x}", source);
        
        // get the size of the current section header's data
        let size = section_header_i.SizeOfRawData as usize;
        //log::info!("Size: {:?}", size);

        // copy section headers into the local process (allocated memory)
        std::ptr::copy_nonoverlapping(
            source as *const std::os::raw::c_void, // this causes problems if it is winapi::ctypes::c_void but ffi works for ffi
            destination as *mut _,
            size,
        )
    }

    image
}

/// Gets exports by hash
fn get_exports_by_hash(module_base: *mut u8, hash: u32) -> Option<*mut u8> {

    // loop through the module exports to find export by hash
    for (name, addr) in unsafe { get_module_exports(module_base) } {
        if fnv1a_hash_32(name.as_bytes()) == hash {
            return Some(addr as _);
        }
    }

    return None;
}

/// Retrieves all function and addresses from the specfied modules
unsafe fn get_module_exports(module_base: *mut u8) -> BTreeMap<String, usize> {
    let mut exports = BTreeMap::new();
    
    let dos_header = module_base as PIMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =  (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS32;

    #[cfg(target_arch = "x86_64")]
    let nt_header = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS64;

    let export_directory = (module_base as usize
        + (*nt_header).OptionalHeader.DataDirectory
            [IMAGE_DIRECTORY_ENTRY_EXPORT as usize]
            .VirtualAddress as usize)
        as PIMAGE_EXPORT_DIRECTORY;

    let names = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfNames as usize)
            as *const u32,
        (*export_directory).NumberOfNames as _,
    );

    let functions = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfFunctions as usize)
            as *const u32,
        (*export_directory).NumberOfFunctions as _,
    );

    let ordinals = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfNameOrdinals as usize)
            as *const u16,
        (*export_directory).NumberOfNames as _,
    );

    //log::info!("[+] Module Base: {:?} Export Directory: {:?} AddressOfNames: {names:p}, AddressOfFunctions: {functions:p}, AddressOfNameOrdinals: {ordinals:p} ", module_base, export_directory);

    for i in 0..(*export_directory).NumberOfNames {

        let name = (module_base as usize + names[i as usize] as usize) as *const i8;

        if let Ok(name) = CStr::from_ptr(name).to_str() {

            let ordinal = ordinals[i as usize] as usize;

            exports.insert(
                name.to_string(),
                module_base as usize + functions[ordinal] as usize,
            );
        }
    }  
    exports
}

/// Gets loaded modules by unique hash
pub unsafe fn get_loaded_module_by_hash(ldr: *mut PEB_LDR_DATA, hash: u32) -> Option<*mut u8> {
	let mut ldr_data_ptr = (*ldr).InLoadOrderModuleList.Flink as PLDR_DATA_TABLE_ENTRY;
	
    while !ldr_data_ptr.is_null() {
		let ldr_data = &*ldr_data_ptr;

		let dll_name = ldr_data.BaseDllName;
		let buffer = dll_name.Buffer;
		
        if buffer.is_null() {
			break;
		}

		let dll_name_wstr = core::slice::from_raw_parts(buffer, dll_name.Length as usize / 2);

		if fnv1a_hash_32_wstr(dll_name_wstr) != hash {
			ldr_data_ptr = ldr_data.InLoadOrderLinks.Flink as PLDR_DATA_TABLE_ENTRY;
			continue;
		}

		return Some(ldr_data.DllBase as _);
	}

	None
}

/* 
unsafe fn rva_to_file_offset_pointer(module_base: usize, mut rva: u32) -> usize {
    let dos_header = module_base as PIMAGE_DOS_HEADER;

    let nt_headers = (module_base as usize + (*dos_header).e_lfanew as usize) as PIMAGE_NT_HEADERS;

    let ref_nt_headers = &*nt_headers;

    let section_header = ((&ref_nt_headers.OptionalHeader as *const _ as usize) 
        + (ref_nt_headers.FileHeader.SizeOfOptionalHeader as usize)) as PIMAGE_SECTION_HEADER;

    let number_of_sections = (*nt_headers).FileHeader.NumberOfSections;
    
    for i in 0..number_of_sections as usize {

        let virt_address = (*section_header.add(i)).VirtualAddress;
        let virt_size = (*section_header.add(i)).Misc.VirtualSize();
        
        if virt_address <= rva && virt_address + virt_size > rva {

            rva -= (*section_header.add(i)).VirtualAddress;
            rva += (*section_header.add(i)).PointerToRawData;
            
            return module_base + rva as usize;
        }
    }

    return 0;
}*/

//https://github.com/Ben-Lichtman/reloader/blob/7d4e82b64f0ee6bf56dec47153721f62e207faa7/src/helpers.rs#L18
pub const fn fnv1a_hash_32_wstr(wchars: &[u16]) -> u32 {
	const FNV_OFFSET_BASIS_32: u32 = 0x811c9dc5;
	const FNV_PRIME_32: u32 = 0x01000193;

	let mut hash = FNV_OFFSET_BASIS_32;

	let mut i = 0;
	while i < wchars.len() {
		let c = unsafe { char::from_u32_unchecked(wchars[i] as u32).to_ascii_lowercase() };
		hash ^= c as u32;
		hash = hash.wrapping_mul(FNV_PRIME_32);
		i += 1;
	}
	hash
}

//https://github.com/Ben-Lichtman/reloader/blob/7d4e82b64f0ee6bf56dec47153721f62e207faa7/src/helpers.rs#L34
pub const fn fnv1a_hash_32(chars: &[u8]) -> u32 {
	const FNV_OFFSET_BASIS_32: u32 = 0x811c9dc5;
	const FNV_PRIME_32: u32 = 0x01000193;

	let mut hash = FNV_OFFSET_BASIS_32;

	let mut i = 0;
	while i < chars.len() {
		let c = unsafe { char::from_u32_unchecked(chars[i] as u32).to_ascii_lowercase() };
		hash ^= c as u32;
		hash = hash.wrapping_mul(FNV_PRIME_32);
		i += 1;
	}
	hash
}

fn get_input() -> std::io::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    Ok(())
}
/// Used for debugging
fn pause() {
    match get_input() {
        Ok(buffer) => println!("{:?}", buffer),
        Err(error) => println!("error: {}", error),
    };
}
