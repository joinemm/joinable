use random_fast_rng::{FastRng, Random};

const ANIMALS: &'static str = include_str!("./lists/animals.txt");
const ADJECTIVES: &'static str = include_str!("./lists/adjectives.txt");

pub fn generate() -> String {
    let mut rng = FastRng::new();
    let adjectives: Vec<&str> = ADJECTIVES.lines().collect();
    let animals: Vec<&str> = ANIMALS.lines().collect();
    let mut result = String::from("");

    for _ in 1..3 {
        result.push_str(adjectives[rng.gen::<usize>() % adjectives.len()]);
    }
    result.push_str(animals[rng.gen::<usize>() % animals.len()]);

    result
}
