#[cfg(feature = "profiler")]
use thread_profiler::profile_scope;

use crate::types::Texture;
use core::{
    hash::Hash,
    iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator},
    ops::{Add, Range},
};
use glsl_layout::*;
use num_traits::PrimInt;
use rendy::{
    factory::Factory,
    hal::{self, buffer::Usage, format, pso, Backend},
    memory::MemoryUsage,
    mesh::VertexFormat,
    resource::{BufferInfo, Escape},
};

#[inline]
pub fn next_range<T: Add<Output = T> + Clone>(prev: &Range<T>, length: T) -> Range<T> {
    prev.end.clone()..prev.end.clone() + length
}

#[inline]
pub fn opt_range<T>(range: Range<T>) -> Range<Option<T>> {
    Some(range.start)..Some(range.end)
}

#[inline]
pub fn usize_range(range: Range<u64>) -> Range<usize> {
    range.start as usize..range.end as usize
}

pub fn ensure_buffer<B: Backend>(
    factory: &Factory<B>,
    buffer: &mut Option<Escape<rendy::resource::Buffer<B>>>,
    usage: Usage,
    memory_usage: impl MemoryUsage,
    min_size: u64,
) -> Result<bool, failure::Error> {
    if buffer.as_ref().map(|b| b.size()).unwrap_or(0) < min_size {
        let new_size = min_size.next_power_of_two();
        let new_buffer = factory.create_buffer(
            BufferInfo {
                size: new_size,
                usage,
            },
            memory_usage,
        )?;
        *buffer = Some(new_buffer);
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn align_size<T: AsStd140>(align: u64, array_len: usize) -> u64
where
    T::Std140: Sized,
{
    let size = (core::mem::size_of::<T::Std140>() * array_len) as u64;
    ((size + align - 1) / align) * align
}

pub fn simple_shader_set<'a, B: Backend>(
    vertex: &'a B::ShaderModule,
    fragment: Option<&'a B::ShaderModule>,
) -> pso::GraphicsShaderSet<'a, B> {
    simple_shader_set_ext(vertex, fragment, None, None, None)
}

pub fn simple_shader_set_ext<'a, B: Backend>(
    vertex: &'a B::ShaderModule,
    fragment: Option<&'a B::ShaderModule>,
    hull: Option<&'a B::ShaderModule>,
    domain: Option<&'a B::ShaderModule>,
    geometry: Option<&'a B::ShaderModule>,
) -> pso::GraphicsShaderSet<'a, B> {
    fn map_entry_point<'a, B: Backend>(module: &'a B::ShaderModule) -> pso::EntryPoint<'a, B> {
        pso::EntryPoint {
            entry: "main",
            module,
            specialization: pso::Specialization::default(),
        }
    }

    pso::GraphicsShaderSet {
        vertex: map_entry_point(vertex),
        fragment: fragment.map(map_entry_point),
        hull: hull.map(map_entry_point),
        domain: domain.map(map_entry_point),
        geometry: geometry.map(map_entry_point),
    }
}

pub fn vertex_desc(
    formats: &[(VertexFormat<'static>, pso::InstanceRate)],
) -> (Vec<pso::VertexBufferDesc>, Vec<pso::AttributeDesc>) {
    let mut vertex_buffers = Vec::with_capacity(formats.len());
    let mut attributes = Vec::with_capacity(formats.len());
    for (format, rate) in formats {
        push_vertex_desc(
            format.gfx_vertex_input_desc(*rate),
            &mut vertex_buffers,
            &mut attributes,
        )
    }
    (vertex_buffers, attributes)
}

pub fn push_vertex_desc(
    (elements, stride, rate): (
        impl IntoIterator<Item = pso::Element<format::Format>>,
        pso::ElemStride,
        pso::InstanceRate,
    ),
    vertex_buffers: &mut Vec<pso::VertexBufferDesc>,
    attributes: &mut Vec<pso::AttributeDesc>,
) {
    let index = vertex_buffers.len() as pso::BufferIndex;
    vertex_buffers.push(pso::VertexBufferDesc {
        binding: index,
        stride,
        rate,
    });

    let mut location = attributes.last().map_or(0, |a| a.location + 1);
    for element in elements.into_iter() {
        attributes.push(pso::AttributeDesc {
            location,
            binding: index,
            element,
        });
        location += 1;
    }
}

#[inline]
pub fn desc_write<'a, B: Backend>(
    set: &'a B::DescriptorSet,
    binding: u32,
    descriptor: pso::Descriptor<'a, B>,
) -> pso::DescriptorSetWrite<'a, B, Option<pso::Descriptor<'a, B>>> {
    pso::DescriptorSetWrite {
        set,
        binding,
        array_offset: 0,
        descriptors: Some(descriptor),
    }
}

#[inline]
pub fn texture_desc<'a, B: Backend>(texture: &'a Texture<B>) -> pso::Descriptor<'a, B> {
    let Texture(inner) = texture;

    pso::Descriptor::CombinedImageSampler(
        inner.view().raw(),
        hal::image::Layout::ShaderReadOnlyOptimal,
        inner.sampler().raw(),
    )
}

pub fn set_layout_bindings(
    bindings: impl IntoIterator<Item = (u32, pso::DescriptorType, pso::ShaderStageFlags)>,
) -> Vec<pso::DescriptorSetLayoutBinding> {
    bindings
        .into_iter()
        .flat_map(|(times, ty, stage_flags)| (0..times).map(move |_| (ty, stage_flags)))
        .enumerate()
        .map(
            |(binding, (ty, stage_flags))| pso::DescriptorSetLayoutBinding {
                binding: binding as u32,
                ty,
                count: 1,
                stage_flags,
                immutable_samplers: false,
            },
        )
        .collect()
}

#[derive(Debug)]
pub struct LookupBuilder<I: Hash + Eq + Copy> {
    forward: fnv::FnvHashMap<I, usize>,
    backward: Vec<I>,
}

impl<I: Hash + Eq + Copy> LookupBuilder<I> {
    pub fn new() -> LookupBuilder<I> {
        LookupBuilder {
            forward: fnv::FnvHashMap::default(),
            backward: Vec::new(),
        }
    }

    pub fn forward(&mut self, id: I) -> usize {
        if let Some(&id_num) = self.forward.get(&id) {
            id_num
        } else {
            let id_num = self.backward.len();
            self.backward.push(id);
            self.forward.insert(id, id_num);
            id_num
        }
    }

    pub fn backward(&self) -> &Vec<I> {
        &self.backward
    }
}

/// Convert any type slice to bytes slice.
pub fn slice_as_bytes<T>(slice: &[T]) -> &[u8] {
    unsafe {
        // Inspecting any value as bytes should be safe.
        core::slice::from_raw_parts(
            slice.as_ptr() as *const u8,
            core::mem::size_of::<T>() * slice.len(),
        )
    }
}

pub fn write_into_slice<I: IntoIterator>(dst_slice: &mut [u8], iter: I) {
    for (data, dst) in iter
        .into_iter()
        .zip(dst_slice.chunks_exact_mut(std::mem::size_of::<I::Item>()))
    {
        dst.copy_from_slice(slice_as_bytes(&[data]));
    }
}

#[must_use = "iterator adaptors are lazy and do nothing unless consumed"]
pub struct TapCountIterator<'a, T: PrimInt, I: Iterator> {
    inner: I,
    counter: &'a mut T,
}

pub trait TapCountIter {
    type Iter: Iterator;
    fn tap_count<'a, T: PrimInt>(self, counter: &'a mut T) -> TapCountIterator<'a, T, Self::Iter>;
}

impl<I: Iterator> TapCountIter for I {
    type Iter = I;
    fn tap_count<'a, T: PrimInt>(self, counter: &'a mut T) -> TapCountIterator<'a, T, I> {
        TapCountIterator {
            inner: self,
            counter,
        }
    }
}

impl<'a, T: PrimInt, I: Iterator> Iterator for TapCountIterator<'a, T, I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        *self.counter = *self.counter + T::one();
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, T: PrimInt, I: DoubleEndedIterator> DoubleEndedIterator for TapCountIterator<'a, T, I> {
    fn next_back(&mut self) -> Option<Self::Item> {
        *self.counter = *self.counter + T::one();
        self.inner.next_back()
    }
}

impl<'a, T: PrimInt, I: ExactSizeIterator> ExactSizeIterator for TapCountIterator<'a, T, I> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a, T: PrimInt, I: FusedIterator> FusedIterator for TapCountIterator<'a, T, I> {}