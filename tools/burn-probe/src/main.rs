// Probe: does Burn's CUDA backend run on this GPU (a GTX 1080 Ti, compute 6.1)?
// Burn 0.22: tensors carry no backend type; the device (CUDA here, with
// autodiff) is chosen at run time.
// 1. a matrix multiply on the GPU, checked against the same product on the CPU;
// 2. 200 Adam steps on a net of the learned tank's shape (143 -> 64 -> 64 -> 8),
//    fitting a fixed random target, the loss printed as it falls.
use burn::module::Module;
use burn::nn::{Linear, LinearConfig, Relu};
use burn::optim::{AdamConfig, GradientsParams};
use burn::tensor::{Device, Distribution, Tensor};

#[derive(Module, Debug)]
struct Mlp {
    a: Linear,
    b: Linear,
    c: Linear,
    act: Relu,
}

impl Mlp {
    fn new(dev: &Device) -> Self {
        Mlp {
            a: LinearConfig::new(143, 64).init(dev),
            b: LinearConfig::new(64, 64).init(dev),
            c: LinearConfig::new(64, 8).init(dev),
            act: Relu::new(),
        }
    }
    fn forward(&self, x: Tensor<2>) -> Tensor<2> {
        let x = self.act.forward(self.a.forward(x));
        let x = self.act.forward(self.b.forward(x));
        self.c.forward(x)
    }
}

fn main() {
    let gpu = Device::cuda(0);

    // 1. matmul, GPU against CPU arithmetic on the same numbers
    let n = 256;
    let x: Tensor<2> = Tensor::random([n, n], Distribution::Uniform(-1.0, 1.0), &gpu);
    let y: Tensor<2> = Tensor::random([n, n], Distribution::Uniform(-1.0, 1.0), &gpu);
    let z = x.clone().matmul(y.clone());
    let xs: Vec<f32> = x.into_data().to_vec().unwrap();
    let ys: Vec<f32> = y.into_data().to_vec().unwrap();
    let zs: Vec<f32> = z.into_data().to_vec().unwrap();
    let mut worst = 0f32;
    for i in 0..n {
        for j in 0..n {
            let mut s = 0f32;
            for k in 0..n { s += xs[i * n + k] * ys[k * n + j]; }
            worst = worst.max((s - zs[i * n + j]).abs());
        }
    }
    println!("matmul {n}x{n} on the GPU: largest difference from the CPU {worst:.2e} -> {}", if worst < 1e-3 { "ok" } else { "WRONG" });

    // 2. training
    let dev = gpu.autodiff();
    let mut model = Mlp::new(&dev);
    let mut opt = AdamConfig::new().init();
    let input: Tensor<2> = Tensor::random([1024, 143], Distribution::Normal(0.0, 1.0), &dev);
    let target: Tensor<2> = Tensor::random([1024, 8], Distribution::Normal(0.0, 1.0), &dev);
    let t0 = std::time::Instant::now();
    let mut first = 0f32;
    let mut last = 0f32;
    for step in 0..200 {
        let out = model.forward(input.clone());
        let loss = (out - target.clone()).powf_scalar(2.0).mean();
        let l: f32 = loss.clone().into_scalar();
        if step == 0 { first = l; }
        last = l;
        if step % 50 == 0 { println!("step {step:3}: loss {l:.4}"); }
        let grads = GradientsParams::from_grads(loss.backward(), &model);
        model = opt.step(1e-3, model, grads);
    }
    println!("200 Adam steps in {:.2} s: loss {first:.4} -> {last:.4} -> {}", t0.elapsed().as_secs_f64(), if last < first * 0.8 { "learning" } else { "NOT LEARNING" });
}
