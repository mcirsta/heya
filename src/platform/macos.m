// Apple platform services. AppKit entry points run only on the main thread;
// Quartz PDF rendering runs in a disposable helper process, without account data.
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

typedef void (*HeyaPanelReply)(uint64_t, const char *const *, size_t);
static NSMutableDictionary<NSNumber *, NSSavePanel *> *panels;

static NSURL *fileURL(const char *path) {
    if (!path || !*path) return nil;
    return [NSURL fileURLWithFileSystemRepresentation:path isDirectory:YES relativeToURL:nil];
}

// kind: 0 save, 1 documents, 2 JPEG/PNG photos, 3 folder.
void heya_panel_begin(uint64_t identifier, uint32_t kind, const char *name,
                      const char *folder, HeyaPanelReply reply) {
    NSCAssert([NSThread isMainThread], @"AppKit requires the main thread");
    @autoreleasepool {
        if (!panels) panels = [NSMutableDictionary new];
        NSSavePanel *panel;
        if (kind == 0) {
            panel = [NSSavePanel savePanel];
            panel.title = @"Save attachment";
            panel.nameFieldStringValue = [NSString stringWithUTF8String:name] ?: @"Attachment";
        } else {
            NSOpenPanel *open = [NSOpenPanel openPanel];
            open.canChooseDirectories = kind == 3;
            open.canChooseFiles = kind != 3;
            open.allowsMultipleSelection = kind != 3;
            open.title = kind == 3 ? @"Choose download folder" :
                         kind == 2 ? @"Choose photos" : @"Choose documents";
            if (kind == 2) {
                // Restrict to the formats the attachment decoder accepts.
                open.allowedContentTypes = @[UTTypeJPEG, UTTypePNG];
                open.allowsOtherFileTypes = NO;
            }
            panel = open;
        }
        panel.canCreateDirectories = YES;
        panel.directoryURL = fileURL(folder);
        panels[@(identifier)] = panel;
        void (^completion)(NSModalResponse) = ^(NSModalResponse response) {
            NSArray<NSURL *> *urls = @[];
            if (response == NSModalResponseOK) {
                urls = kind == 0 ? (panel.URL ? @[panel.URL] : @[]) : ((NSOpenPanel *)panel).URLs;
            }
            if (urls.count > 100) {
                reply(identifier, NULL, SIZE_MAX);
            } else {
                const char *paths[100];
                for (NSUInteger i = 0; i < urls.count; ++i) {
                    paths[i] = urls[i].fileSystemRepresentation;
                }
                // URL storage stays alive through this synchronous callback.
                reply(identifier, paths, urls.count);
            }
            [panels removeObjectForKey:@(identifier)];
        };
        NSWindow *parent = NSApp.mainWindow;
        if (parent && ![parent isKindOfClass:[NSPanel class]]) {
            [panel beginSheetModalForWindow:parent completionHandler:completion];
        } else {
            [panel beginWithCompletionHandler:completion];
        }
    }
}

void heya_panel_cancel(uint64_t identifier) {
    NSCAssert([NSThread isMainThread], @"AppKit requires the main thread");
    [panels[@(identifier)] cancel:nil];
}

// Return a bounded, white-backed top strip of page one as PNG. CoreGraphics
// handles crop boxes and rotations; ImageIO encodes the result entirely in RAM.
int heya_pdf_render(const uint8_t *bytes, size_t length, uint32_t width,
                    uint32_t height, size_t limit, uint8_t **output, size_t *size) {
    @autoreleasepool {
        CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, bytes, length, NULL);
        if (!provider) return 0;
        CGPDFDocumentRef document = CGPDFDocumentCreateWithProvider(provider);
        CGDataProviderRelease(provider);
        if (!document) return 0;
        CGPDFPageRef page = CGPDFDocumentGetPage(document, 1);
        if (!page) { CGPDFDocumentRelease(document); return 0; }
        CGRect crop = CGPDFPageGetBoxRect(page, kCGPDFCropBox);
        int rotation = CGPDFPageGetRotationAngle(page) % 180;
        CGFloat pageWidth = rotation ? crop.size.height : crop.size.width;
        CGFloat pageHeight = rotation ? crop.size.width : crop.size.height;
        if (!isfinite(pageWidth) || !isfinite(pageHeight) || pageWidth <= 0 || pageHeight <= 0) {
            CGPDFDocumentRelease(document); return 0;
        }
        CGFloat scaledHeight = ceil(pageHeight * width / pageWidth);
        if (!isfinite(scaledHeight) || scaledHeight > 1000000) {
            CGPDFDocumentRelease(document); return 0;
        }
        CGColorSpaceRef color = CGColorSpaceCreateDeviceRGB();
        CGContextRef context = CGBitmapContextCreate(NULL, width, height, 8, width * 4,
            color, kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(color);
        if (!context) { CGPDFDocumentRelease(document); return 0; }
        CGContextSetRGBFillColor(context, 1, 1, 1, 1);
        CGContextFillRect(context, CGRectMake(0, 0, width, height));
        CGContextTranslateCTM(context, 0, height - scaledHeight);
        CGContextConcatCTM(context, CGPDFPageGetDrawingTransform(page, kCGPDFCropBox,
            CGRectMake(0, 0, width, scaledHeight), 0, true));
        CGContextDrawPDFPage(context, page);
        CGPDFDocumentRelease(document);
        CGImageRef image = CGBitmapContextCreateImage(context);
        CGContextRelease(context);
        if (!image) return 0;
        CFMutableDataRef data = CFDataCreateMutable(NULL, 0);
        CGImageDestinationRef destination = CGImageDestinationCreateWithData(data,
            CFSTR("public.png"), 1, NULL);
        bool ok = false;
        if (destination) {
            CGImageDestinationAddImage(destination, image, NULL);
            ok = CGImageDestinationFinalize(destination);
            CFRelease(destination);
        }
        CGImageRelease(image);
        CFIndex count = CFDataGetLength(data);
        if (ok && count > 0 && (size_t)count <= limit) {
            *output = malloc((size_t)count);
            ok = *output != NULL;
            if (ok) {
                memcpy(*output, CFDataGetBytePtr(data), (size_t)count);
                *size = (size_t)count;
            }
        } else { ok = false; }
        CFRelease(data);
        return ok ? 1 : 0;
    }
}

void heya_native_free(void *bytes) { free(bytes); }
