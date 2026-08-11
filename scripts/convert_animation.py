import argparse
import sys
from PIL import Image

def process_gif(input_path, output_path, invert=False):
    try:
        img = Image.open(input_path)
    except IOError:
        print(f"Error: Could not open {input_path}")
        sys.exit(1)

    if img.format != 'GIF':
        print(f"Warning: {input_path} is not a GIF. Attempting to process anyway...")

    frames = []
    
    try:
        while True:
            # Convert frame to 1-bit monochrome (using dither if needed, though for pure black/white it's direct)
            # '1' mode in PIL: 0 is black, 255 is white.
            frame = img.copy().convert('1')
            
            # Ensure it is 128x64
            if frame.size != (128, 64):
                frame = frame.resize((128, 64), Image.Resampling.NEAREST)
            
            frame_data = bytearray()
            
            # embedded-graphics ImageRaw format:
            # 1 bit per pixel, packed horizontally. MSB is the leftmost pixel.
            # 128 pixels / 8 = 16 bytes per row.
            
            pixels = frame.load()
            for y in range(64):
                for x_byte in range(16):
                    byte_val = 0
                    for bit in range(8):
                        x = x_byte * 8 + bit
                        # PIL '1' mode: white is 255, black is 0.
                        # For OLED: typically On (1) is white, Off (0) is black.
                        is_white = pixels[x, y] > 127
                        
                        pixel_on = is_white
                        if invert:
                            pixel_on = not pixel_on
                            
                        if pixel_on:
                            byte_val |= (1 << (7 - bit))
                            
                    frame_data.append(byte_val)
                    
            frames.append(frame_data)
            img.seek(img.tell() + 1)
    except EOFError:
        pass # End of GIF

    print(f"Processed {len(frames)} frames.")
    
    with open(output_path, 'wb') as f:
        for frame in frames:
            f.write(frame)
            
    print(f"Successfully saved {output_path} ({len(frames) * 1024} bytes)")

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Convert GIF to embedded-graphics 1-bit raw binary format.")
    parser.add_argument("input", help="Input GIF file")
    parser.add_argument("output", help="Output .bin file")
    parser.add_argument("--invert", action="store_true", help="Invert the colors (swap black and white)")
    
    args = parser.parse_args()
    process_gif(args.input, args.output, args.invert)
