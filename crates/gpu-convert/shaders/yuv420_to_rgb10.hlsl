// Decoded 4:2:0 10-bit (BT.709, FULL range) -> R'G'B' for a R10G10B10A2_UNORM target.
//
// Must match the CPU reference in crates/color (subsample.rs: yuv420p10_rgb_at,
// yuv420p10_to_rgb10). Output is 1:1: one pixel shader invocation per picture pixel.
//
// Inputs (decoder output, P010 layout: 10-bit code in the top bits of 16):
//   t0 Luma    R16_UNORM     W x H
//   t1 Chroma  R16G16_UNORM  W/2 x H/2  (Cb, Cr)
//
// Chroma siting "left": horizontally co-sited with even luma columns, vertically
// at luma y = 2j + 0.5. Bilinear upsampling, edges clamp:
//   x even: chroma column x/2;              x odd: halfway between x/2 and x/2+1
//   y even: rows y/2-1, y/2 weights .25/.75; y odd: rows y/2, y/2+1 weights .75/.25

Texture2D<float>  Luma   : register(t0);
Texture2D<float2> Chroma : register(t1);

static const float KR = 0.2126;
static const float KB = 0.0722;
static const float KG = 1.0 - KR - KB;
static const float MAX_CODE = 1023.0;
static const float CHROMA_MID = 512.0;

struct VsOut { float4 pos : SV_Position; };

VsOut VS(uint id : SV_VertexID)
{
    // Full-screen triangle: (-1,-1) (-1,3) (3,-1)
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

// UNORM16 sample -> exact 10-bit code (word >> 6).
float code_of(float unorm16)
{
    uint word = (uint)(unorm16 * 65535.0 + 0.5);
    return (float)(word >> 6);
}

float2 chroma_at(int2 c, int2 cmax)
{
    float2 v = Chroma.Load(int3(clamp(c, int2(0, 0), cmax), 0));
    return (float2(code_of(v.x), code_of(v.y)) - CHROMA_MID) / MAX_CODE;
}

float4 PS(VsOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    uint cw, ch;
    Chroma.GetDimensions(cw, ch);
    int2 cmax = int2(cw, ch) - 1;

    int i0 = p.x >> 1;
    float fx = (p.x & 1) ? 0.5 : 0.0;
    int j0;
    float fy;
    if ((p.y & 1) == 0) { j0 = (p.y >> 1) - 1; fy = 0.75; }
    else                { j0 = p.y >> 1;       fy = 0.25; }

    float2 r0 = (1.0 - fx) * chroma_at(int2(i0, j0), cmax)     + fx * chroma_at(int2(i0 + 1, j0), cmax);
    float2 r1 = (1.0 - fx) * chroma_at(int2(i0, j0 + 1), cmax) + fx * chroma_at(int2(i0 + 1, j0 + 1), cmax);
    float2 pbpr = (1.0 - fy) * r0 + fy * r1;

    float y = code_of(Luma.Load(int3(p, 0))) / MAX_CODE;
    float r = y + 2.0 * (1.0 - KR) * pbpr.y;
    float b = y + 2.0 * (1.0 - KB) * pbpr.x;
    float g = (y - KR * r - KB * b) / KG;

    // Quantize here so the UNORM10 conversion of code / 1023 is exact.
    float3 code = floor(saturate(float3(r, g, b)) * MAX_CODE + 0.5);
    return float4(code / MAX_CODE, 1.0);
}
