struct Settings { v: array<vec4<f32>, 17>, }
@group(0) @binding(0) var<uniform> settings: Settings;
@group(0) @binding(1) var<storage, read> front: array<u32>;
@group(0) @binding(2) var<storage, read> rear: array<u32>;
@group(0) @binding(3) var<storage, read_write> output: array<u32>;
@group(0) @binding(4) var<storage, read> output_rays: array<vec2<f32>>;
fn rotate(ray:vec3<f32>, base:u32)->vec3<f32> {
    return vec3(dot(settings.v[base].xyz,ray),dot(settings.v[base+1].xyz,ray),dot(settings.v[base+2].xyz,ray));
}
fn project(ray:vec3<f32>,base:u32)->vec3<f32> {
    let p=rotate(ray,base);let angle=acos(clamp(p.z/length(p),-1.,1.));
    let lens=settings.v[base+3];let k=settings.v[base+4];let tang=settings.v[base+5];
    let denom=p.z+k.w*length(p);
    if angle>tang.z || denom<=0.000001 {return vec3(0.,0.,-1.);}
    let xy=p.xy/denom;let x=xy.x;let y=xy.y;let r=dot(xy,xy);
    let radial=1.+r*(k.x+r*(k.y+r*k.z));
    let distorted=xy*radial+vec2(2.*tang.x*x*y+tang.y*(r+2.*x*x),2.*tang.y*x*y+tang.x*(r+2.*y*y));
    let uv=lens.zw+lens.xy*distorted;
    if any(uv<vec2(0.)) || any(uv>vec2(1.)) || dot(uv-vec2(0.5),uv-vec2(0.5))>0.25 {return vec3(0.,0.,-1.);}
    return vec3(uv,angle);
}
fn unpack(pixel:u32)->vec3<f32> {return vec3(f32(pixel&255u),f32((pixel>>8u)&255u),f32((pixel>>16u)&255u));}
fn at(x:u32,y:u32,lens:u32)->vec3<f32> {
    let index=y*u32(settings.v[16].x)+x;
    if lens==0u {return unpack(front[index]);}return unpack(rear[index]);
}
fn sample(uv:vec2<f32>,lens:u32)->vec3<f32> {
    let dim=settings.v[16].xy;let p=clamp(uv*dim-vec2(0.5),vec2(0.),dim-vec2(1.));
    let a=vec2<u32>(floor(p));let b=min(a+vec2(1u),vec2<u32>(dim)-vec2(1u));let f=fract(p);
    return mix(mix(at(a.x,a.y,lens),at(b.x,a.y,lens),f.x),mix(at(a.x,b.y,lens),at(b.x,b.y,lens),f.x),f.y);
}
@compute @workgroup_size(16,16)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=settings.v[0].xy;if id.x>=u32(size.x) || id.y>=u32(size.y) {return;}
    let xy=output_rays[id.y*u32(size.x)+id.x]*settings.v[0].z;
    let ray=rotate(normalize(vec3(xy,1.)),1u);let a=project(ray,4u);let b=project(ray,10u);
    var color=vec3(0.);
    if a.z>=0. && b.z>=0. {
        var weight=select(0.,1.,a.z<=b.z);
        let width=settings.v[0].w;
        if width>0. {
            weight=smoothstep(-width*0.5,width*0.5,b.z-a.z);
            let front_coverage=max(0.,min(settings.v[9].z-a.z,(0.5-length(a.xy-vec2(0.5)))*3.14159265359));
            let rear_coverage=max(0.,min(settings.v[15].z-b.z,(0.5-length(b.xy-vec2(0.5)))*3.14159265359));
            let total=weight*front_coverage+(1.-weight)*rear_coverage;
            if total>0.000000000001 {weight=weight*front_coverage/total;}
        }
        color=mix(sample(b.xy,1u),sample(a.xy,0u),weight);
    } else if a.z>=0. {color=sample(a.xy,0u);} else if b.z>=0. {color=sample(b.xy,1u);}
    let rgb=vec3<u32>(clamp(round(color),vec3(0.),vec3(255.)));
    output[id.y*u32(size.x)+id.x]=rgb.x|(rgb.y<<8u)|(rgb.z<<16u)|0xff000000u;
}
