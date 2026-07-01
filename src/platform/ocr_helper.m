// Native ObjC OCR helper — avoids objc2 msg_send! type-encoding issues
#import <Vision/Vision.h>
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <string.h>

const char* clippi_nsimage_to_png_base64(const void* image_ptr, int target_size) {
    @autoreleasepool {
        if (!image_ptr) {
            return NULL;
        }

        NSImage *image = (__bridge NSImage *)image_ptr;
        int size = target_size > 0 ? target_size : 32;
        NSRect sourceRect = NSMakeRect(0, 0, image.size.width, image.size.height);
        CGImageRef cgImage = [image CGImageForProposedRect:&sourceRect context:nil hints:nil];
        if (!cgImage) {
            return NULL;
        }

        NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
            initWithBitmapDataPlanes:NULL
                          pixelsWide:size
                          pixelsHigh:size
                       bitsPerSample:8
                     samplesPerPixel:4
                            hasAlpha:YES
                            isPlanar:NO
                      colorSpaceName:NSCalibratedRGBColorSpace
                         bytesPerRow:0
                        bitsPerPixel:0];
        if (!rep) {
            return NULL;
        }

        NSGraphicsContext *ctx = [NSGraphicsContext graphicsContextWithBitmapImageRep:rep];
        if (!ctx) {
            return NULL;
        }
        [NSGraphicsContext saveGraphicsState];
        [NSGraphicsContext setCurrentContext:ctx];
        ctx.imageInterpolation = NSImageInterpolationHigh;
        [NSColor.clearColor set];
        NSRectFill(NSMakeRect(0, 0, size, size));
        [image drawInRect:NSMakeRect(0, 0, size, size)
                 fromRect:NSZeroRect
                operation:NSCompositingOperationSourceOver
                 fraction:1.0];
        [NSGraphicsContext restoreGraphicsState];

        NSData *pngData = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        if (!pngData || pngData.length == 0) {
            return NULL;
        }

        NSString *base64 = [pngData base64EncodedStringWithOptions:0];
        if (!base64) {
            return NULL;
        }

        return strdup(base64.UTF8String);
    }
}

const char* clippi_ocr_recognize(const char* image_path) {
    @autoreleasepool {
        NSString *path = [NSString stringWithUTF8String:image_path];
        if (!path) {
            return NULL;
        }

        // Load image via NSImage (most reliable cross-format loader)
        NSImage *nsImage = [[NSImage alloc] initWithContentsOfFile:path];
        if (!nsImage || !nsImage.isValid) {
            return NULL;
        }

        // Get CGImage from NSImage
        NSRect rect = NSMakeRect(0, 0, nsImage.size.width, nsImage.size.height);
        CGImageRef cgImage = [nsImage CGImageForProposedRect:&rect context:nil hints:nil];
        if (!cgImage) {
            // Fallback: render via NSBitmapImageRep
            NSData *tiffData = [nsImage TIFFRepresentation];
            if (!tiffData) {
                return NULL;
            }
            NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithData:tiffData];
            if (!rep) {
                return NULL;
            }
            cgImage = [rep CGImage];
        }
        if (!cgImage) {
            return NULL;
        }

        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:cgImage options:@{}];

        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;
        request.recognitionLanguages = @[@"zh-Hans", @"zh-Hant", @"en"];

        NSError *error = nil;
        BOOL success = [handler performRequests:@[request] error:&error];

        if (!success) {
            NSString *msg = error ? error.localizedDescription : @"unknown error";
            NSLog(@"[OCR] performRequests failed: %@", msg);
            return NULL;
        }

        NSArray *results = request.results;
        if (results.count == 0) {
            return strdup("");
        }

        NSMutableString *text = [NSMutableString string];
        for (NSUInteger i = 0; i < results.count; i++) {
            VNRecognizedTextObservation *obs = results[i];
            NSArray<VNRecognizedText *> *candidates = [obs topCandidates:1];
            if (candidates.count > 0) {
                if (text.length > 0) [text appendString:@"\n"];
                [text appendString:candidates.firstObject.string];
            }
        }

        return strdup([text UTF8String]);
    }
}

/// Free the string returned by clippi_ocr_recognize
void clippi_ocr_free_string(const char* s) {
    if (s) free((void*)s);
}
