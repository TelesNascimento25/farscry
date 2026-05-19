use crate::Classifier;
use farscry_core::TextRegion;
use std::time::Instant;

fn create_test_regions(count: usize) -> Vec<TextRegion> {
    (0..count)
        .map(|i| TextRegion {
            text: format!("Region {}", i),
            cx: (i % 10) as f32 * 100.0,
            cy: (i / 10) as f32 * 50.0,
            w: 80.0,
            h: 20.0,
        })
        .collect()
}

#[test]
fn benchmark_20_elements() {
    let classifier = Classifier;
    let regions = create_test_regions(20);

    let ocr_output = farscry_core::OcrOutput {
        regions: regions.clone(),
        width: 800,
        height: 600,
    };

    let _ = farscry_core::ElementClassifier::classify(&classifier, &ocr_output);

    let start = Instant::now();
    let elements = farscry_core::ElementClassifier::classify(&classifier, &ocr_output);
    let elapsed = start.elapsed();

    println!("Classification time for 20 elements: {:?}", elapsed);
    println!("Elements classified: {}", elements.len());

    assert!(
        elapsed.as_millis() < 2);
}

#[test]
fn benchmark_50_elements() {
    let classifier = Classifier;
    let regions = create_test_regions(50);

    let ocr_output = farscry_core::OcrOutput {
        regions: regions.clone(),
        width: 800,
        height: 600,
    };

    let _ = farscry_core::ElementClassifier::classify(&classifier, &ocr_output);

    let start = Instant::now();
    let _ = farscry_core::ElementClassifier::classify(&classifier, &ocr_output);
    let elapsed = start.elapsed();

    println!("Classification time for 50 elements: {:?}", elapsed);
}
