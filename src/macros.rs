#[macro_export]
macro_rules! timeit {
    ($label:expr, $block:block) => {{
        use log::debug;
        use std::time::Instant;

        let start = Instant::now();
        let result = { $block };
        let elapsed = start.elapsed();
        debug!("⏱️ [{}] took: {:.3?}", $label, elapsed);
        (result, elapsed)
    }};
}

#[macro_export]
macro_rules! prompt_args {
    ( $( $key:expr => $val:expr ),* $(,)? ) => {{
        let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        $(
            map.insert($key.to_string(), $val.to_string());
        )*
        map
    }};
}

