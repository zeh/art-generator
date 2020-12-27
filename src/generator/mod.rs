use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;

use image::{DynamicImage, Rgb, RgbImage};

use painter::Painter;
use utils::formatting::format_time;
use utils::image::{color_transform, diff, scale_image};
use utils::numbers::AverageNumber;
use utils::terminal;

pub mod painter;
pub mod utils;

pub enum ProcessResult {
	// Image generated and sent along with its diff value
	Ok(RgbImage, f64),
	// Image generated, but we now its diff is not better than the current one, so we don't send anything
	Ignore,
	// Could not generate image because of an error
	Error(String),
}

pub struct ProcessCallbackResult<'a> {
	pub is_success: bool,
	pub is_final: bool,
	pub num_tries: u32,
	pub num_generations: u32,
	pub diff: f64,
	pub time_elapsed: f32,
	pub metadata: HashMap<String, String>,
	pub image: &'a RgbImage,
}

/// A definition for the image generation. This will contain all data needed for a generation process.
pub struct Generator {
	target: RgbImage,
	current: RgbImage,
}

impl Generator {
	pub fn from_image(target_image: DynamicImage, scale: f64) -> Generator {
		let mut target = target_image.to_rgb8();
		if scale != 1.0f64 {
			target = scale_image(&target, scale);
		}
		let current = RgbImage::new(target.dimensions().0, target.dimensions().1);
		Generator {
			target: target,
			current: current,
		}
	}

	pub fn from_image_and_matrix(target_image: DynamicImage, scale: f64, matrix: [f64; 12]) -> Generator {
		let mut target = target_image.to_rgb8();
		if scale != 1.0f64 {
			target = scale_image(&target, scale);
		}
		let current = RgbImage::new(target.dimensions().0, target.dimensions().1);
		Generator {
			target: color_transform(&target, matrix),
			current: current,
		}
	}

	pub fn prepopulate_with_image(&mut self, current_image: DynamicImage) {
		self.current = current_image.to_rgb8();
	}

	pub fn prepopulate_with_color(&mut self, r: u8, g: u8, b: u8) {
		let dimensions = self.current.dimensions();
		self.current = RgbImage::from_pixel(dimensions.0, dimensions.1, Rgb([r, g, b]))
	}

	pub fn process(
		&mut self,
		target_tries: u32,
		target_generations: u32,
		target_diff: f64,
		candidates: usize,
		painter: impl Painter + Send + Sync + 'static,
		cb: Option<&dyn Fn(ProcessCallbackResult)>,
	) {
		let mut curr_diff = diff(&self.current, &self.target);

		println!("Starting tries; initial difference from target is {:.2}%.", curr_diff * 100.0);

		let mut used;

		let time_started = Instant::now();
		let mut time_elapsed_paint = 0;
		let mut time_elapsed_diff = 0;
		let mut time_started_try;
		let mut time_elapsed_try = 0;

		let mut curr_tries: u32 = 0;
		let mut curr_generations: u32 = 0;

		let mut total_processes: u32 = 0;

		let arc_painter = Arc::new(painter);
		let arc_target = Arc::new(self.target.clone());

		let mut time_elapsed_try_avg = AverageNumber::new(100);
		let mut time_elapsed_generation_avg = AverageNumber::new(50);
		let mut time_elapsed_diff_pct_avg = AverageNumber::new(50);

		println!("First try...");

		let mut time_started_generation = Instant::now();
		let mut time_started_diff = Instant::now();
		let mut diff_last_generation = curr_diff;

		let mut time_last_print = Instant::now();

		loop {
			time_started_try = Instant::now();
			used = false;

			if candidates == 1 {
				// Simple path with no concurrency
				let time_started_paint = Instant::now();
				let new_candidate =
					arc_painter.paint(&self.current, total_processes, &self.target).expect("painting");
				time_elapsed_paint += time_started_paint.elapsed().as_micros();

				let time_started_diff = Instant::now();
				let new_diff = diff(&new_candidate, &self.target);
				time_elapsed_diff += time_started_diff.elapsed().as_micros();

				if new_diff < curr_diff {
					self.current = new_candidate;
					curr_diff = new_diff;
					used = true;
				}

				total_processes = total_processes.wrapping_add(1);
			} else {
				// Complex path with concurrency
				// This can sometimes be slower because of all the image cloning done,
				// and will use more memory, but on the aggregate reaches successfull
				// generations in about 25% of the original time
				let (tx, rx) = mpsc::channel();

				for candidate in 0..candidates {
					let tx1 = mpsc::Sender::clone(&tx);
					let thread_painter = Arc::clone(&arc_painter);
					let thread_current = self.current.clone();
					let thread_target = Arc::clone(&arc_target);

					thread::spawn(move || {
						let result = match thread_painter.paint(
							&thread_current,
							total_processes.wrapping_add(candidate as u32),
							&thread_target,
						) {
							Ok(new_candidate) => {
								let new_diff = diff(&new_candidate, &thread_target);

								// Only report candidates that are actually better than the current diff,
								// to minimize the back-and-forth of data. To be fair, however, this doesn't
								// seem to to do much in terms of performance.
								if new_diff < curr_diff {
									ProcessResult::Ok(new_candidate, new_diff)
								} else {
									ProcessResult::Ignore
								}
							}
							Err(err) => ProcessResult::Error(err.to_owned()),
						};
						tx1.send(result).unwrap();
					});
				}

				drop(tx);

				for result in rx {
					match result {
						ProcessResult::Ok(new_candidate, new_diff) => {
							if new_diff < curr_diff {
								self.current = new_candidate;
								curr_diff = new_diff;
								used = true;
							}
						}
						ProcessResult::Ignore => {}
						ProcessResult::Error(err) => {
							panic!("{}", err);
						}
					}
				}

				total_processes = total_processes.wrapping_add(candidates as u32);
			}

			if used {
				curr_generations += 1;

				// Update time stats for generation
				time_elapsed_generation_avg.put(time_started_generation.elapsed().as_micros() as f64);
				time_started_generation = Instant::now();

				// Update time stats for diff
				let diff_change = diff_last_generation - curr_diff;
				let diff_time = time_started_diff.elapsed().as_micros() as f64;
				let diff_time_per_pct = diff_time / diff_change;
				time_elapsed_diff_pct_avg.put(diff_time_per_pct);
				diff_last_generation = curr_diff;
				time_started_diff = Instant::now();
			}

			curr_tries += 1;

			let finished = (target_tries > 0 && curr_tries == target_tries)
				|| (target_generations > 0 && curr_generations == target_generations)
				|| (target_diff > 0.0 && curr_diff <= target_diff);

			if let Some(process_callback) = cb {
				process_callback(ProcessCallbackResult {
					is_success: used,
					is_final: finished,
					num_tries: curr_tries,
					num_generations: curr_generations,
					diff: curr_diff,
					time_elapsed: time_started.elapsed().as_secs_f32(),
					metadata: arc_painter.get_metadata(),
					image: &self.current,
				});
			}

			// Update time stats for tries
			time_elapsed_try += time_started_try.elapsed().as_micros();
			time_elapsed_try_avg.put(time_started_try.elapsed().as_micros() as f64);

			// Only output log if the generation succeeded, or if enough time has passed
			if used || time_last_print.elapsed().as_secs() >= 1 {
				terminal::cursor_up();
				terminal::erase_line_to_end();

				// Tries block
				if target_tries > 0 {
					let remaining = target_tries - curr_tries;
					let time_left = remaining as f64 * time_elapsed_try_avg.get().unwrap();
					print!(
						"Try {}/{} ({} left): ",
						curr_tries,
						target_tries,
						format_time(time_left / 1000.0)
					);
				} else {
					print!("Try {}: ", curr_tries);
				}

				// Generations block
				if target_generations > 0 {
					let remaining = target_generations - curr_generations;
					let time_left = remaining as f64 * time_elapsed_generation_avg.get().unwrap();
					print!(
						"{}/{} generations so far ({} left), ",
						curr_generations,
						target_generations,
						format_time(time_left / 1000.0)
					);
				} else {
					print!("{} generations so far, ", curr_generations);
				}

				// Diff block
				if target_diff > 0.0 {
					let remaining = curr_diff - target_diff;
					let time_left = remaining as f64 * time_elapsed_diff_pct_avg.get().unwrap();
					println!(
						"new difference is {:.2}%/{:.2}% ({} left)",
						curr_diff * 100.0,
						target_diff * 100.0,
						format_time(time_left / 1000.0)
					);
				} else {
					println!("new difference is {:.2}%", curr_diff * 100.0);
				}

				time_last_print = Instant::now();
			}

			if finished {
				// Requirements reached, can stop trying
				break;
			}
		}

		let time_elapsed = time_started.elapsed().as_secs_f32();
		let atts = curr_tries as f64 * 1000.0;

		let final_diff = diff(&self.current, &self.target);
		println!(
			"Finished {} tries in {:.3}s ({:.3}ms avg per try), using {} candidate threads.",
			curr_tries,
			time_elapsed,
			time_elapsed_try as f64 / atts,
			candidates
		);
		if candidates == 1 {
			println!(
				"Tries took an average of {:.3}ms for painting, and {:.3}ms for diffing, using a single thread.",
				time_elapsed_paint as f64 / atts,
				time_elapsed_diff as f64 / atts
			);
		}
		println!(
			"Produced {} generations, a {:.2}% success rate.",
			curr_generations,
			curr_generations as f64 / curr_tries as f64 * 100.0
		);
		println!("The final difference from target is {:.2}%.", final_diff * 100.0);
	}
}
