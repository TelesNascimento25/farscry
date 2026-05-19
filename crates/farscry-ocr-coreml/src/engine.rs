use farscry_core::{FarscryError, OcrEngine, OcrOutput};
use image::DynamicImage;
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    AnyThread,
};
use objc2_core_ml::{
    MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel, MLMultiArray,
    MLMultiArrayDataType,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::dict::get_character_dict;
use crate::postprocess::{boxes_to_regions, ctc_decode, db_postprocess};
use crate::preprocess::{
    normalize_recognition_to_tensor, normalize_to_tensor, resize_for_detection,
    resize_for_recognition, LimitType,
};

struct OcrRequest {
    image: DynamicImage,
    response_sender: mpsc::Sender<OcrResponse>,
}

struct OcrResponse {
    result: Result<OcrOutput, FarscryError>,
}

pub struct CoreMlOcrEngine {
    sender: SyncSender<OcrRequest>,
    _handle: JoinHandle<()>,
}

impl CoreMlOcrEngine {
    pub fn from_models_dir(models_dir: &Path) -> Result<Self, FarscryError> {
        use crate::model::ensure_compiled;
        use crate::verify::verify_models;

        verify_models(models_dir)?;

        let det_path = ensure_compiled(models_dir, "farscry-det")?;
        let rec_path = ensure_compiled(models_dir, "farscry-rec")?;

        Self::from_model_paths(det_path, rec_path)
    }

    pub fn from_model_paths(
        det_model_path: PathBuf,
        rec_model_path: PathBuf,
    ) -> Result<Self, FarscryError> {
        let (request_sender, request_receiver): (SyncSender<OcrRequest>, Receiver<OcrRequest>) =
            mpsc::sync_channel(32);

        let handle = std::thread::spawn(move || {
            inference_thread(request_receiver, det_model_path, rec_model_path);
        });

        Ok(Self {
            sender: request_sender,
            _handle: handle,
        })
    }

    #[allow(clippy::new_ret_no_self)]
    pub fn new(det_model_path: PathBuf, rec_model_path: PathBuf) -> Result<Self, FarscryError> {
        Self::from_model_paths(det_model_path, rec_model_path)
    }
}

impl OcrEngine for CoreMlOcrEngine {
    fn extract(&self, image: &DynamicImage) -> Result<OcrOutput, FarscryError> {
        let (response_sender, response_receiver) = mpsc::channel();

        let request = OcrRequest {
            image: image.clone(),
            response_sender,
        };

        self.sender
            .send(request)
            .map_err(|e| FarscryError::OcrFailed(format!("Failed to send OCR request: {}", e)))?;

        let response = response_receiver.recv().map_err(|e| {
            FarscryError::OcrFailed(format!("Failed to receive OCR response: {}", e))
        })?;

        response.result
    }
}

fn load_model(model_path: &Path) -> Result<Retained<MLModel>, FarscryError> {
    let path_str = model_path
        .to_str()
        .ok_or_else(|| FarscryError::OcrFailed(format!("Invalid model path: {:?}", model_path)))?;

    unsafe {
        let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(path_str));
        let config = objc2_core_ml::MLModelConfiguration::new();
        config.setComputeUnits(objc2_core_ml::MLComputeUnits::All);
        MLModel::modelWithContentsOfURL_configuration_error(&url, &config)
            .map_err(|e| FarscryError::OcrFailed(format!("Failed to load CoreML model: {:?}", e)))
    }
}

fn inference_thread(
    receiver: Receiver<OcrRequest>,
    det_model_path: PathBuf,
    rec_model_path: PathBuf,
) {
    let det_model = match load_model(&det_model_path) {
        Ok(model) => model,
        Err(e) => {
            eprintln!("Failed to load detection model: {:?}", e);
            return;
        }
    };

    let rec_model = match load_model(&rec_model_path) {
        Ok(model) => model,
        Err(e) => {
            eprintln!("Failed to load recognition model: {:?}", e);
            return;
        }
    };

    let character_dict = get_character_dict();

    while let Ok(request) = receiver.recv() {
        let result = run_ocr_pipeline(&request.image, &det_model, &rec_model, &character_dict);
        let _ = request.response_sender.send(OcrResponse { result });
    }
}

fn run_ocr_pipeline(
    image: &DynamicImage,
    det_model: &MLModel,
    rec_model: &MLModel,
    character_dict: &[String],
) -> Result<OcrOutput, FarscryError> {
    let start = Instant::now();

    let rgb_image = image.to_rgb8();

    let (resized_det, scale_info) = resize_for_detection(&rgb_image, 960, LimitType::Max);
    let det_tensor = normalize_to_tensor(&resized_det);

    let det_output = run_detection_inference(det_model, &det_tensor)?;

    let det_output_array4 = if det_output.ndim() == 4 {
        det_output
            .into_dimensionality::<ndarray::Ix4>()
            .map_err(|e| FarscryError::OcrFailed(format!("Failed to convert to 4D: {:?}", e)))?
    } else {
        return Err(FarscryError::OcrFailed(format!(
            "Expected 4D detection output, got {}D",
            det_output.ndim()
        )));
    };

    let boxes = db_postprocess(&det_output_array4, vec![scale_info], 0.3, 0.6, 2.0);
    let det_boxes = &boxes[0];

    let cropped_regions: Vec<_> = det_boxes
        .iter()
        .map(|box_| crop_region(&rgb_image, box_))
        .collect();

    let rec_tensors: Vec<ndarray::Array3<f32>> = cropped_regions
        .iter()
        .map(|img| {
            let (resized, _) = resize_for_recognition(img, 48, 320);

            normalize_recognition_to_tensor(&resized, 320).index_axis_move(ndarray::Axis(0), 0)
        })
        .collect();

    let mut all_texts: Vec<String> = Vec::new();
    let mut all_scores: Vec<f32> = Vec::new();
    const REC_BATCH: usize = 32;

    for chunk in rec_tensors.chunks(REC_BATCH) {
        let chunk_size = chunk.len();
        let chunk_output = run_recognition_batch(rec_model, chunk, chunk_size)?;
        let (chunk_texts, chunk_scores) = ctc_decode(&chunk_output, character_dict);
        all_texts.extend(chunk_texts);
        all_scores.extend(chunk_scores);
    }

    let (texts, scores) = (all_texts, all_scores);

    let regions = boxes_to_regions(det_boxes, &texts, &scores);

    let elapsed = start.elapsed();
    let width = rgb_image.width();
    let height = rgb_image.height();

    eprintln!(
        "[farscry] OCR pipeline completed in {:.2}ms",
        elapsed.as_secs_f64() * 1000.0
    );

    Ok(OcrOutput {
        regions,
        width,
        height,
    })
}

pub(crate) fn run_detection_inference(
    model: &MLModel,
    tensor: &ndarray::Array4<f32>,
) -> Result<ndarray::ArrayD<f32>, FarscryError> {
    let input_array = ndarray_to_mlarray(tensor)?;

    let input_provider = unsafe {
        let key = NSString::from_str("x");
        let val = MLFeatureValue::featureValueWithMultiArray(&input_array);

        let val_any: &AnyObject = &*(val.as_ref() as *const MLFeatureValue as *const AnyObject);
        let key_ref: &NSString = &key;
        let dict: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices::<NSString>(&[key_ref], &[val_any]);

        MLDictionaryFeatureProvider::initWithDictionary_error(
            MLDictionaryFeatureProvider::alloc(),
            &dict,
        )
    }
    .map_err(|e| {
        FarscryError::OcrFailed(format!(
            "Failed to create MLDictionaryFeatureProvider: {:?}",
            e
        ))
    })?;

    let output = unsafe {
        let opts = objc2_core_ml::MLPredictionOptions::new();

        let input: &MLDictionaryFeatureProvider = input_provider.as_ref();
        let input_proto: &ProtocolObject<dyn MLFeatureProvider> = ProtocolObject::from_ref(input);
        model.predictionFromFeatures_options_error(input_proto, &opts)
    }
    .map_err(|e| FarscryError::OcrFailed(format!("Detection inference failed: {:?}", e)))?;

    let output_value = unsafe { output.featureValueForName(&NSString::from_str("var_2219")) }
        .ok_or_else(|| {
            FarscryError::OcrFailed("Detection output 'var_2219' not found".to_string())
        })?;

    let output_array = unsafe { output_value.multiArrayValue() }.ok_or_else(|| {
        FarscryError::OcrFailed("Detection output is not an MLMultiArray".to_string())
    })?;

    let output_array_data = mlarray_to_ndarray(&output_array)?;
    Ok(output_array_data)
}

pub(crate) fn run_recognition_batch(
    model: &MLModel,
    crops: &[ndarray::Array3<f32>],
    num_real: usize,
) -> Result<ndarray::Array3<f32>, FarscryError> {
    const REC_BATCH: usize = 32;
    const SEQ: usize = 40;
    const VOCAB: usize = 438;
    const C: usize = 3;
    const H: usize = 48;
    const W: usize = 320;

    if num_real == 0 {
        return Ok(ndarray::Array3::zeros((0, SEQ, VOCAB)));
    }

    let mut batch = ndarray::Array4::<f32>::zeros((REC_BATCH, C, H, W));
    for (i, crop) in crops.iter().enumerate().take(REC_BATCH) {
        batch.index_axis_mut(ndarray::Axis(0), i).assign(crop);
    }

    let input_array = ndarray_to_mlarray(&batch)?;
    let output = unsafe {
        let key = NSString::from_str("x");
        let val = MLFeatureValue::featureValueWithMultiArray(&input_array);
        let val_any: &AnyObject = &*(val.as_ref() as *const MLFeatureValue as *const AnyObject);
        let dict = NSDictionary::from_slices::<NSString>(&[key.as_ref()], &[val_any]);
        let prov = MLDictionaryFeatureProvider::initWithDictionary_error(
            MLDictionaryFeatureProvider::alloc(),
            &dict,
        )
        .map_err(|e| FarscryError::OcrFailed(format!("RecBatch provider: {:?}", e)))?;
        let opts = objc2_core_ml::MLPredictionOptions::new();
        let prov_ref: &MLDictionaryFeatureProvider = prov.as_ref();
        let proto: &ProtocolObject<dyn MLFeatureProvider> = ProtocolObject::from_ref(prov_ref);
        model
            .predictionFromFeatures_options_error(proto, &opts)
            .map_err(|e| FarscryError::OcrFailed(format!("RecBatch inference: {:?}", e)))?
    };

    let output_value = unsafe {
        let mut found = None;

        for name in &["var_2343", "fetch_name_0"] {
            if let Some(v) = output.featureValueForName(&NSString::from_str(name)) {
                found = Some(v);
                break;
            }
        }
        if found.is_none() {
            for i in 2200usize..=2500 {
                if let Some(v) =
                    output.featureValueForName(&NSString::from_str(&format!("var_{i}")))
                {
                    found = Some(v);
                    break;
                }
            }
        }
        found
    }
    .ok_or_else(|| FarscryError::OcrFailed("RecBatch: output tensor not found".into()))?;

    let ml_arr = unsafe { output_value.multiArrayValue() }
        .ok_or_else(|| FarscryError::OcrFailed("RecBatch: output is not MLMultiArray".into()))?;

    let full = mlarray_to_ndarray(&ml_arr)?;
    let arr3 = match full.ndim() {
        3 => full
            .into_dimensionality::<ndarray::Ix3>()
            .map_err(|e| FarscryError::OcrFailed(format!("RecBatch 3D: {:?}", e)))?,
        4 => {
            let (d0, d1, d2, d3) = (
                full.shape()[0],
                full.shape()[1],
                full.shape()[2],
                full.shape()[3],
            );
            #[allow(deprecated)]
            let reshaped = full
                .into_shape((d0, d2 * d3, d1))
                .map_err(|e| FarscryError::OcrFailed(format!("RecBatch 4D reshape: {:?}", e)))?
                .into_dimensionality::<ndarray::Ix3>()
                .map_err(|e| FarscryError::OcrFailed(format!("RecBatch 4D 3D: {:?}", e)))?;
            reshaped
        }
        _ => {
            return Err(FarscryError::OcrFailed(format!(
                "RecBatch unexpected ndim={}",
                full.ndim()
            )))
        }
    };

    Ok(arr3.slice_move(ndarray::s![..num_real, .., ..]))
}

pub(crate) fn ndarray_to_mlarray(
    tensor: &ndarray::Array4<f32>,
) -> Result<Retained<MLMultiArray>, FarscryError> {
    let (batch, channels, height, width) = tensor.dim();
    let shape = [batch, channels, height, width];

    unsafe {
        let ns_shape: Vec<Retained<NSNumber>> = shape
            .iter()
            .map(|&d| NSNumber::numberWithInteger(d as isize))
            .collect();
        let ns_shape_refs: Vec<&NSNumber> = ns_shape.iter().map(|n| n.as_ref()).collect();
        let shape_array = NSArray::from_slice(&ns_shape_refs);

        let arr = MLMultiArray::initWithShape_dataType_error(
            MLMultiArray::alloc(),
            &shape_array,
            MLMultiArrayDataType::Float32,
        )
        .map_err(|e| FarscryError::OcrFailed(format!("MLMultiArray init failed: {:?}", e)))?;

        #[allow(deprecated)]
        let count = arr.count() as usize;
        #[allow(deprecated)]
        let ptr = arr.dataPointer().as_ptr() as *mut f32;
        let data_slice = std::slice::from_raw_parts_mut(ptr, count);

        let contiguous_tensor = tensor.as_standard_layout();
        let tensor_slice = contiguous_tensor.as_slice().ok_or_else(|| {
            FarscryError::OcrFailed("tensor layout is not contiguous".to_string())
        })?;
        data_slice.copy_from_slice(tensor_slice);

        Ok(arr)
    }
}

fn mlarray_to_ndarray(array: &MLMultiArray) -> Result<ndarray::ArrayD<f32>, FarscryError> {
    unsafe {
        let shape = array.shape();
        let ndim = shape.len();

        if !(3..=4).contains(&ndim) {
            return Err(FarscryError::OcrFailed(format!(
                "Expected 3D or 4D array, got {}D",
                ndim
            )));
        }

        let dims: Vec<usize> = (0..ndim)
            .map(|i| shape.objectAtIndex(i).integerValue() as usize)
            .collect();

        let ml_strides_ns = array.strides();
        let ml_strides: Vec<usize> = (0..ndim)
            .map(|i| ml_strides_ns.objectAtIndex(i).integerValue() as usize)
            .collect();

        let storage_len = ml_strides[0] * dims[0];
        #[allow(deprecated)]
        let ptr = array.dataPointer().as_ptr() as *const f32;
        let raw = std::slice::from_raw_parts(ptr, storage_len);

        let total_elems: usize = dims.iter().product();
        let mut compact = vec![0.0f32; total_elems];

        match ndim {
            3 => {
                for d0 in 0..dims[0] {
                    for d1 in 0..dims[1] {
                        for d2 in 0..dims[2] {
                            let src = d0 * ml_strides[0] + d1 * ml_strides[1] + d2 * ml_strides[2];
                            let dst = d0 * (dims[1] * dims[2]) + d1 * dims[2] + d2;
                            compact[dst] = raw[src];
                        }
                    }
                }
            }
            4 => {
                for d0 in 0..dims[0] {
                    for d1 in 0..dims[1] {
                        for d2 in 0..dims[2] {
                            for d3 in 0..dims[3] {
                                let src = d0 * ml_strides[0]
                                    + d1 * ml_strides[1]
                                    + d2 * ml_strides[2]
                                    + d3 * ml_strides[3];
                                let dst = d0 * (dims[1] * dims[2] * dims[3])
                                    + d1 * (dims[2] * dims[3])
                                    + d2 * dims[3]
                                    + d3;
                                compact[dst] = raw[src];
                            }
                        }
                    }
                }
            }
            _ => unreachable!(),
        }

        let array_view =
            ndarray::ArrayView::from_shape(dims.as_slice(), &compact).map_err(|e| {
                FarscryError::OcrFailed(format!("Failed to create ndarray view: {:?}", e))
            })?;

        Ok(array_view.to_owned())
    }
}

fn crop_region(image: &image::RgbImage, box_: &crate::postprocess::BoundingBox) -> image::RgbImage {
    use oar_ocr_core::processors::{BoundingBox as OarBBox, Point};

    let oar_bbox = OarBBox {
        points: box_
            .points
            .iter()
            .map(|(x, y)| Point { x: *x, y: *y })
            .collect(),
    };

    match oar_ocr_core::utils::BBoxCrop::crop_bounding_box(image, &oar_bbox) {
        Ok(cropped) => cropped,
        Err(_) => {
            let (min_x, max_x) = box_
                .points
                .iter()
                .fold((f32::MAX, f32::MIN), |(mn, mx), (x, _)| {
                    (mn.min(*x), mx.max(*x))
                });
            let (min_y, max_y) = box_
                .points
                .iter()
                .fold((f32::MAX, f32::MIN), |(mn, mx), (_, y)| {
                    (mn.min(*y), mx.max(*y))
                });
            let mut img_copy = image.clone();
            image::imageops::crop(
                &mut img_copy,
                min_x as u32,
                min_y as u32,
                (max_x - min_x) as u32,
                (max_y - min_y) as u32,
            )
            .to_image()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_integration_with_actual_models() {
        let current_dir = std::env::current_dir().expect("Could not get current directory");

        let repo_root = current_dir
            .ancestors()
            .nth(2)
            .expect("Could not find repo root")
            .to_path_buf();

        let models_dir = repo_root.join("spike-native-coreml").join("models");
        let det_model = models_dir.join("pp-ocrv5_mobile_det.mlmodelc");
        let rec_model = models_dir.join("en_pp-ocrv5_mobile_rec.mlmodelc");
        let test_image = repo_root.join("spike").join("test.png");

        eprintln!("Current directory: {}", current_dir.display());
        eprintln!("Repo root: {}", repo_root.display());
        eprintln!("Detection model: {}", det_model.display());
        eprintln!("Recognition model: {}", rec_model.display());
        eprintln!("Test image: {}", test_image.display());

        if !det_model.exists() {
            panic!(
                "Integration test: detection model not found at {}",
                det_model.display()
            );
        }

        if !rec_model.exists() {
            panic!(
                "Integration test: recognition model not found at {}",
                rec_model.display()
            );
        }

        if !test_image.exists() {
            panic!(
                "Integration test: test image not found at {}",
                test_image.display()
            );
        }

        eprintln!("Loading CoreML OCR engine...");
        let engine = CoreMlOcrEngine::new(det_model, rec_model).expect("Failed to load OCR engine");
        eprintln!("CoreML OCR engine loaded successfully");

        eprintln!("Loading test image...");
        let image = image::open(&test_image).expect("Failed to load test image");
        eprintln!("Test image loaded: {}x{}", image.width(), image.height());

        eprintln!("Running CoreML OCR...");
        let start = std::time::Instant::now();
        let result = engine.extract(&image).expect("OCR extraction failed");
        let elapsed = start.elapsed();
        eprintln!("OCR completed in {:.2}ms", elapsed.as_secs_f64() * 1000.0);

        eprintln!("OCR found {} text regions", result.regions.len());
        eprintln!("Image dimensions: {}x{}", result.width, result.height);

        for (i, region) in result.regions.iter().enumerate() {
            eprintln!(
                "Region {}: '{}' at ({:.1}, {:.1})",
                i, region.text, region.cx, region.cy
            );
        }

        assert!(!result.regions.is_empty());
        assert!(result.width > 0);
        assert!(result.height > 0);

        for region in &result.regions {
            assert!(!region.text.trim().is_empty());
        }

        eprintln!("\n=== CoreML Integration Test PASSED ===");
    }
}
