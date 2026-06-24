pub trait ImageEncoder {
    fn encode(&mut self, rgba: &[u8], width: u32, height: u32) -> &[u8];
}
