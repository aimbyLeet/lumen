// A monitor's picture (R10G10B10A2, with mips) scaled into the viewport: the
// composite view of several monitors (apps/client/src/composite.rs). Not 1:1:
// trilinear, so text does not shimmer when the picture is much smaller.

Texture2D<float4> Picture : register(t0);
SamplerState Linear : register(s0);

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut VS(uint id : SV_VertexID)
{
    // Full-viewport triangle: uv (0,0) top-left .. (1,1) bottom-right.
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = uv;
    return o;
}

float4 PS(VsOut i) : SV_Target
{
    return float4(Picture.Sample(Linear, i.uv).rgb, 1.0);
}
