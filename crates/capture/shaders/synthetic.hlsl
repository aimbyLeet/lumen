// Synthetic desktop-like test frame. Output is sRGB-encoded values written
// straight into a B8G8R8A8_UNORM render target (no sRGB view), exactly what
// Desktop Duplication would deliver.
//
// Pure function of (pixel x, pixel y, frame number): no state, no time.
// Layout (pixels, top-left origin):
//   (0,0)           barcode, one row of 16x16 cells (see capture::barcode)
//   background      slowly moving smooth gradients (banding check)
//   (32,48)         640x240 colour patches: primaries/secondaries, grey ramp, skin tones
//   (32,320)        640x240 fine detail: 1-px lines and 2x2 checkerboards, scrolling 1 px/frame
//   moving box      200x200 solid box travelling across the screen
//   (W-288,48)      256x256 per-frame hash noise (encoder stress)
// Everything but the barcode is drawn as of frame `frame_no_in` inside `window`
// (x0, y0, x1, y1) and as of `still_frame` outside it: a still picture with a
// moving part.

cbuffer Params : register(b0)
{
    uint frame_no_in;
    uint width;
    uint height;
    uint cell_count;
    uint4 bits;   // barcode cells, cell i = bit (i % 32) of component (i / 32)
    uint4 window; // x0, y0, x1, y1
    uint still_frame;
    uint3 pad;
};

struct VsOut { float4 pos : SV_Position; };

VsOut VS(uint id : SV_VertexID)
{
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

uint hash(uint x)
{
    // PCG hash
    uint state = x * 747796405u + 2891336453u;
    uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

bool inside(int2 p, int2 origin, int2 size)
{
    int2 d = p - origin;
    return d.x >= 0 && d.y >= 0 && d.x < size.x && d.y < size.y;
}

static const float3 PATCHES[8] = {
    float3(1, 0, 0), float3(0, 1, 0), float3(0, 0, 1), float3(0, 1, 1),
    float3(1, 0, 1), float3(1, 1, 0), float3(1, 1, 1), float3(0, 0, 0)
};

// Skin-like tones (sRGB 8-bit values / 255).
static const float3 SKIN[8] = {
    float3(255, 224, 189) / 255.0, float3(241, 194, 125) / 255.0,
    float3(224, 172, 105) / 255.0, float3(198, 134, 66) / 255.0,
    float3(141, 85, 36) / 255.0,   float3(92, 56, 31) / 255.0,
    float3(233, 173, 150) / 255.0, float3(176, 112, 96) / 255.0
};

float3 detail(int2 d, int block, uint frame_no)
{
    // block 0: red on white, 1: blue on black, 2: magenta on green
    float3 fg, bg;
    if (block == 0) { fg = float3(1, 0, 0); bg = float3(1, 1, 1); }
    else if (block == 1) { fg = float3(0, 0, 1); bg = float3(0, 0, 0); }
    else { fg = float3(1, 0, 1); bg = float3(0, 1, 0); }
    int sx = d.x + (int)frame_no;   // scroll 1 px per frame
    bool on;
    if (d.x < 160)
        on = (sx & 1) != 0;                          // vertical 1-px lines
    else if (d.x < 320)
        on = ((d.y & 1) != 0);                       // horizontal 1-px lines
    else if (d.x < 480)
        on = (((sx >> 1) + (d.y >> 1)) & 1) != 0;    // 2x2 checkerboard
    else
        on = ((sx + d.y) & 1) != 0;                  // 1-px checkerboard
    return on ? fg : bg;
}

float4 PS(VsOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    uint frame = inside(p, int2(window.xy), int2(window.zw - window.xy)) ? frame_no_in : still_frame;
    float2 uv = float2(p) / float2(width, height);
    float t = frame / 600.0;

    // Background: smooth gradients, low contrast so 8-bit banding is visible.
    float3 col;
    col.r = 0.30 + 0.25 * sin(6.2831853 * (uv.x * 0.6 + uv.y * 0.2 - t));
    col.g = 0.30 + 0.25 * sin(6.2831853 * (uv.y * 0.5 - uv.x * 0.1 + t * 1.5) + 2.0);
    col.b = 0.35 + 0.25 * sin(6.2831853 * (uv.x * 0.3 + uv.y * 0.4 + t * 0.5) + 4.0);

    // Colour patches.
    int2 d = p - int2(32, 48);
    if (inside(p, int2(32, 48), int2(640, 80)))
        col = PATCHES[d.x / 80];
    else if (inside(p, int2(32, 128), int2(640, 80)))
        col = (float)(d.x / 40) / 15.0;            // 16-step grey ramp
    else if (inside(p, int2(32, 208), int2(640, 80)))
        col = SKIN[d.x / 80];

    // Fine detail, three chroma-critical combinations.
    for (int b = 0; b < 3; b++) {
        int2 o = int2(32, 320 + b * 80);
        if (inside(p, o, int2(640, 80)))
            col = detail(p - o, b, frame);
    }

    // Moving box.
    int travel = (int)width + 200;
    int bx = (int)((frame * 7u) % (uint)travel) - 200;
    int by = (int)(height * 0.6 + 150.0 * sin(frame * 0.05));
    if (inside(p, int2(bx, by), int2(200, 200)))
        col = float3(0.95, 0.55, 0.10);

    // Per-frame noise patch.
    int2 no = int2((int)width - 288, 48);
    if (inside(p, no, int2(256, 256))) {
        uint h = hash((uint)(p.x - no.x) + 256u * (uint)(p.y - no.y) + 65536u * frame);
        col = float3(h & 255u, (h >> 8) & 255u, (h >> 16) & 255u) / 255.0;
    }

    // Barcode (top-left, last so nothing overwrites it).
    if (p.y < 16 && p.x < (int)cell_count * 16) {
        uint c = (uint)p.x / 16u;
        uint w = c >> 5;
        uint v = w == 0 ? bits.x : (w == 1 ? bits.y : (w == 2 ? bits.z : bits.w));
        col = ((v >> (c & 31u)) & 1u) ? 1.0 : 0.0;
    }

    return float4(saturate(col), 1.0);
}
