// Reference render of a .myb by the real libmypaint (1.6), the same strokes
// as `cargo run --release -p aurastudio-brush --example myb_compare`, to
// compare Forge with MyPaint. Linux: apt install libmypaint-dev, then
//   gcc -O2 ref.c -o ref -I/usr/include/libmypaint -lmypaint -lm
//   ./ref brush.myb ref.ppm
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>
#include <libmypaint/mypaint-brush.h>
#include <libmypaint/mypaint-fixed-tiled-surface.h>
#define W 640
#define H 220
static char *slurp(const char *p){FILE*f=fopen(p,"rb");fseek(f,0,2);long n=ftell(f);rewind(f);char*b=malloc(n+1);if(fread(b,1,n,f)!=(size_t)n)exit(1);b[n]=0;fclose(f);return b;}
static void band(MyPaintTiledSurface *t){
  for(int ty=0;ty*64<H;ty++)for(int tx=0;tx*64<W;tx++){
    MyPaintTileRequest r; mypaint_tile_request_init(&r,0,tx,ty,FALSE);
    mypaint_tiled_surface_tile_request_start(t,&r);
    for(int y=0;y<64;y++)for(int x=0;x<64;x++){int X=tx*64+x,Y=ty*64+y; uint16_t*p=r.buffer+(y*64+x)*4;
      if(Y>=120&&Y<200){ p[0]=(uint16_t)((X%256)/255.0*32768); p[1]=(uint16_t)(0.45*32768); p[2]=(uint16_t)((255-X%256)/255.0*32768); p[3]=32768; }
      else { p[0]=p[1]=p[2]=p[3]=0; } }
    mypaint_tiled_surface_tile_request_end(t,&r);
  }
}
int main(int argc,char**argv){
  MyPaintBrush*b=mypaint_brush_new(); mypaint_brush_from_defaults(b);
  if(!mypaint_brush_from_string(b,slurp(argv[1]))){fprintf(stderr,"parse fail\n");return 1;}
  mypaint_brush_set_base_value(b,MYPAINT_BRUSH_SETTING_COLOR_H,0.62f);
  mypaint_brush_set_base_value(b,MYPAINT_BRUSH_SETTING_COLOR_S,0.8f);
  mypaint_brush_set_base_value(b,MYPAINT_BRUSH_SETTING_COLOR_V,0.55f);
  MyPaintFixedTiledSurface*fs=mypaint_fixed_tiled_surface_new(W,H);
  MyPaintSurface*s=(MyPaintSurface*)fs;
  band((MyPaintTiledSurface*)fs);
  for(int k=0;k<2;k++){
    mypaint_brush_reset(b); mypaint_brush_new_stroke(b);
    int n=k==0?160:60;
    for(int i=0;i<=n;i++){
      float t=(float)i/n, x,y,p;
      if(k==0){x=40+t*560; y=80-50*sinf(t*6.2831853f); p=fmaxf(sinf(t*3.1415926f),0.05f);}
      else {x=40+t*560; y=160; p=0.8f;}
      mypaint_surface_begin_atomic(s);
      mypaint_brush_stroke_to(b,s,x,y,p,0,0,i==0?1.0:0.008);
      mypaint_surface_end_atomic(s,NULL);
    }
  }
  FILE*o=fopen(argv[2],"wb"); fprintf(o,"P6\n%d %d\n255\n",W,H);
  MyPaintTiledSurface*t=(MyPaintTiledSurface*)fs;
  unsigned char*img=malloc(W*H*3);
  for(int ty=0;ty*64<H;ty++)for(int tx=0;tx*64<W;tx++){
    MyPaintTileRequest r; mypaint_tile_request_init(&r,0,tx,ty,TRUE);
    mypaint_tiled_surface_tile_request_start(t,&r);
    for(int y=0;y<64;y++)for(int x=0;x<64;x++){int X=tx*64+x,Y=ty*64+y; if(X>=W||Y>=H)continue; uint16_t*p=r.buffer+(y*64+x)*4;
      double a=p[3]/32768.0; for(int c=0;c<3;c++){ double v=p[c]/32768.0 + (1-a); img[(Y*W+X)*3+c]=(unsigned char)fmin(255,fmax(0,v*255+0.5)); } }
    mypaint_tiled_surface_tile_request_end(t,&r);
  }
  fwrite(img,1,W*H*3,o); fclose(o); return 0;
}
