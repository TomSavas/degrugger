use crate::src_file::SrcFile;

//pub struct Point<'a> {
#[derive(Debug)]
pub struct Point<'a> {
    pub addr: u64, 
    //old_instruction: u8,

    //redirected_addr: u64,
    //injected_instruction: u8,
    //injected_code: Vec<u8>,

    pub enabled: bool,

    // Temp

    //file: &'a SrcFile,
    //file: Path,
    pub line_number: u64,
    //file_checksum: Hash, // For checking if file is outdated, etc

    //group: Group, 

    //stats: InsertPointStats,
    phantom: std::marker::PhantomData<&'a ()>,
}

//pub struct InsertPointGroup {
//    name: String,
//    color: [u8, 3],
//    insertpoints: Vec<&dyn InsertPoint>,
//}

impl<'a> Point<'a> {
    //pub fn new(file: &'a SrcFile, line_number: u64) -> Self {
    // TEMP: do not pass in address, it's not valid until a run is started
    pub fn new(addr: u64, line_number: u64) -> Self {
        Point {
            addr: addr,
            //old_instruction: 0,

            //redirected_addr: 0,
            //injected_instruction: 0xcc,
            //injected_code: vec![],

            enabled: true,

            //file: file,
            line_number: line_number,

            //group: Group, 

            //stats: InsertPointStats,
            phantom: std::marker::PhantomData,
        }
    }
}

trait InsertPoint {
    fn enabled(&self) -> bool;
    fn set_enable(&mut self, enable: bool);
}

#[derive(Debug)]
pub struct BreakPoint<'a> {
    pub point: Point<'a>,

    //contition: TriggerCondition,
    //note: String,
}

impl<'a> BreakPoint<'a> {
    pub fn new(point: Point<'a>) -> Self {
        BreakPoint{ point: point }
    }
}

impl<'a> InsertPoint for BreakPoint<'a> {
    fn enabled(&self) -> bool {
        self.point.enabled
    }

    fn set_enable(&mut self, enable: bool) {
        self.point.enabled = enable;
    }
}
