// BGRA8 (sRGB-encoded desktop) -> P010, BT.709 matrix, FULL range, 10 bit, 4:2:0.
//
// Must match the CPU reference in crates/color (ycbcr.rs + subsample.rs).
// Two passes over a full-screen triangle:
//   PS_Luma   -> RTV R16_UNORM     on the P010 texture (luma plane, W x H)
//   PS_Chroma -> RTV R16G16_UNORM  on the P010 texture (chroma plane, W/2 x H/2)
//
// Chroma siting: "left" (HEVC chroma_sample_loc_type 0 / AV1 CSP_VERTICAL):
// horizontally co-sited with even luma columns, vertically between rows.
// Filter: rows {2j, 2j+1} x cols {2i-1, 2i, 2i+1} with weights 1/2 x {1/4,1/2,1/4}.

Texture2D<float4> Src : register(t0);

static const float KR = 0.2126;
static const float KB = 0.0722;
static const float KG = 1.0 - KR - KB;
static const float MAX_CODE = 1023.0;
static const float CHROMA_MID = 512.0;
// P010 keeps the 10-bit code in the top bits: word = code << 6.
// The RTV is UNORM16, so we write word / 65535.
static const float P010_SCALE = 64.0 / 65535.0;

struct VsOut { float4 pos : SV_Position; };

VsOut VS(uint id : SV_VertexID)
{
    // Full-screen triangle: (-1,-1) (-1,3) (3,-1)
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

float quantize(float v)
{
    return clamp(floor(v + 0.5), 0.0, MAX_CODE);
}

float PS_Luma(VsOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    float3 rgb = Src.Load(int3(p, 0)).rgb;
    float y = dot(rgb, float3(KR, KG, KB));
    return quantize(MAX_CODE * y) * P010_SCALE;
}

float2 PS_Chroma(VsOut i) : SV_Target
{
    uint w, h;
    Src.GetDimensions(w, h);
    int2 c = int2(i.pos.xy);          // chroma sample index (i, j)
    int x0 = 2 * c.x;
    int y0 = 2 * c.y;
    int ym = (int)h - 1;
    int xm = (int)w - 1;

    float3 acc = 0;
    [unroll] for (int dy = 0; dy < 2; ++dy)
    {
        int yy = min(y0 + dy, ym);
        acc += 0.125 * Src.Load(int3(clamp(x0 - 1, 0, xm), yy, 0)).rgb;
        acc += 0.250 * Src.Load(int3(min(x0, xm),          yy, 0)).rgb;
        acc += 0.125 * Src.Load(int3(min(x0 + 1, xm),      yy, 0)).rgb;
    }

    float y  = dot(acc, float3(KR, KG, KB));
    float pb = (acc.b - y) / (2.0 * (1.0 - KB));
    float pr = (acc.r - y) / (2.0 * (1.0 - KR));
    float cb = quantize(MAX_CODE * pb + CHROMA_MID);
    float cr = quantize(MAX_CODE * pr + CHROMA_MID);
    return float2(cb, cr) * P010_SCALE;
}
