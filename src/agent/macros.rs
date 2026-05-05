#[macro_export]
macro_rules! llm_retry {
    ($operation:expr, $cnt:expr) => {{
        use std::process::exit;
        let mut retry_cnt = 0;
        loop {
            if retry_cnt >= $cnt {
                exit(1)
            }
            let ret = $operation;
            if ret.is_ok() {
                break ret.unwrap();
            }
            warn!("LLM response failed with error:{:?}", ret);
            retry_cnt += 1;
        }
    }};
}
