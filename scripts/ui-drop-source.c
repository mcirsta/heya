/* Synthetic Xdnd source for the private Xvfb clipboard/drop check. */
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static void send_message(Display *d, Window target, Atom type, long a, long b, long c, long e, long f) {
    XEvent event = {0};
    event.xclient.type=ClientMessage; event.xclient.display=d;
    event.xclient.window=target; event.xclient.message_type=type; event.xclient.format=32;
    event.xclient.data.l[0]=a;event.xclient.data.l[1]=b;event.xclient.data.l[2]=c;event.xclient.data.l[3]=e;event.xclient.data.l[4]=f;
    XSendEvent(d,target,False,NoEventMask,&event);XFlush(d);
}
int main(int argc,char **argv) {
    if(argc!=3)return 2;
    Display *d=XOpenDisplay(NULL);if(!d)return 3;
    Window target=strtoul(argv[1],NULL,10), source=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,1,1,0,0,0);
    Atom uri=XInternAtom(d,"text/uri-list",False),selection=XInternAtom(d,"XdndSelection",False);
    Atom status=XInternAtom(d,"XdndStatus",False),finished=XInternAtom(d,"XdndFinished",False);
    XSetSelectionOwner(d,selection,source,CurrentTime);
    send_message(d,target,XInternAtom(d,"XdndEnter",False),source,5L<<24,uri,0,0);
    send_message(d,target,XInternAtom(d,"XdndPosition",False),source,0,(600L<<16)|400,CurrentTime,XInternAtom(d,"XdndActionCopy",False));
    int dropped=0,accepted=0,served=0,result=4;
    for(int ticks=0;ticks<500;ticks++) {
        while(XPending(d)) {
            XEvent event;XNextEvent(d,&event);
            if(event.type==ClientMessage && event.xclient.message_type==status && !dropped) {
                /* A receiver may defer acceptance until its async URI read completes. */
                accepted=(event.xclient.data.l[1]&1)!=0;
            } else if(event.type==SelectionRequest) {
                XSelectionRequestEvent *r=&event.xselectionrequest;
                Atom property=r->property?r->property:r->target;
                XChangeProperty(d,r->requestor,property,uri,8,PropModeReplace,(unsigned char*)argv[2],strlen(argv[2]));
                XEvent answer={0};answer.xselection.type=SelectionNotify;answer.xselection.display=d;
                answer.xselection.requestor=r->requestor;answer.xselection.selection=r->selection;
                answer.xselection.target=r->target;answer.xselection.property=property;answer.xselection.time=r->time;
                XSendEvent(d,r->requestor,False,NoEventMask,&answer);XFlush(d);served=1;
            } else if(event.type==ClientMessage && event.xclient.message_type==finished) {
                result=event.xclient.data.l[1]&1?0:6;goto done;
            }
        }
        if(accepted && served && !dropped) {
            dropped=1;send_message(d,target,XInternAtom(d,"XdndDrop",False),source,0,CurrentTime,0,0);
        } else if(!dropped && ticks%10==0) {
            send_message(d,target,XInternAtom(d,"XdndPosition",False),source,0,(600L<<16)|400,CurrentTime,XInternAtom(d,"XdndActionCopy",False));
        }
        struct pollfd fd={ConnectionNumber(d),POLLIN,0};poll(&fd,1,10);
    }
done:
    XDestroyWindow(d,source);XCloseDisplay(d);return result;
}
