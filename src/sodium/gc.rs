/*
 * A Pure Reference Counting Garbage Collector
 * DAVID F. BACON, CLEMENT R. ATTANASIO, V.T. RAJAN, STEPHEN E. SMITH
 */

use std::marker::PhantomData;
use std::cell::Cell;
use std::ptr;
use std::ops::Deref;
use std::any::Any;

pub struct GcCtx {
    roots: Vec<*mut Node>,
    auto_collect_cycles_on_decrement: bool
}

pub struct Gc<A: ?Sized> {
    ctx: *mut GcCtx,
    node: *mut Node,
    phantom: PhantomData<A>
}

impl<A: ?Sized> Clone for Gc<A> {
    fn clone(&self) -> Self {
        let ctx = unsafe { &mut *self.ctx };
        ctx.increment(self.node);
        Gc {
            ctx: self.ctx,
            node: self.node,
            phantom: PhantomData
        }
    }
}

impl<A: ?Sized> Drop for Gc<A> {
    fn drop(&mut self) {
        let ctx = unsafe { &mut *self.ctx };
        ctx.decrement(self.node);
        if ctx.auto_collect_cycles_on_decrement {
            ctx.collect_cycles();
        }
    }
}

impl<A: Any> Deref for Gc<A> {
    type Target = A;

    fn deref(&self) -> &A {
        let node = unsafe { &mut *self.node };
        let data: &Box<Any> = unsafe { &node.data };
        let value: &A = match data.downcast_ref::<A>() {
            Some(value2) => value2,
            None => panic!()
        };
        value
    }
}

impl<A: ?Sized> Gc<A> {
    pub fn downgrade(&self) -> GcWeak<A> {
        let weak_node = Box::into_raw(Box::new(
            WeakNode {
                node: Some(self.node)
            }
        ));
        GcWeak {
            ctx: self.ctx,
            weak_node: weak_node,
            phantom: PhantomData
        }
    }
}

pub struct GcWeak<A: ?Sized> {
    ctx: *mut GcCtx,
    weak_node: *mut WeakNode,
    phantom: PhantomData<A>
}

impl<A: ?Sized> Drop for GcWeak<A> {
    fn drop(&mut self) {
        unsafe { Box::from_raw(self.weak_node); }
    }
}

impl<A: ?Sized> GcWeak<A> {
    pub fn upgrade(&self) -> Option<Gc<A>> {
        let weak_node = unsafe { &*self.weak_node };
        weak_node.node.map(|node| {
            let ctx = unsafe { &mut *self.ctx };
            ctx.increment(node);
            Gc {
                ctx: self.ctx,
                node: node,
                phantom: PhantomData
            }
        })
    }
}

impl<A: ?Sized> Gc<A> {
    pub fn add_child<B>(&self, child: &Gc<B>) {
        let node = unsafe { &mut *self.node };
        let child_node = child.node;
        if !node.children.contains(&child_node) {
            node.children.push(child_node);
        }
    }

    pub fn remove_child<B>(&self, child: &Gc<B>) {
        let node = unsafe { &mut *self.node };
        let child_node = child.node;
        node.children.retain(|c| !ptr::eq(*c, child_node));
    }
}

#[derive(PartialEq)]
enum Colour {
    Black,
    Purple,
    White,
    Gray
}

struct Node {
    count: i32,
    colour: Colour,
    buffered: bool,
    children: Vec<*mut Node>,
    weak_nodes: Vec<*mut WeakNode>,
    data: Box<Any>
}

impl Drop for Node {
    fn drop(&mut self) {
        for weak_node in &self.weak_nodes {
            let weak_node = unsafe { &mut **weak_node };
            weak_node.node = None;
        }
    }
}

struct WeakNode {
    node: Option<*mut Node>
}

impl Drop for WeakNode {
    fn drop(&mut self) {
        match &self.node {
            &Some(ref node) => {
                let node = unsafe { &mut **node };
                node.weak_nodes.retain(|weak_node| !ptr::eq(*weak_node, self));
            },
            &None => ()
        }
    }
}

impl GcCtx {

    pub fn new() -> GcCtx {
        GcCtx {
            roots: Vec::new(),
            auto_collect_cycles_on_decrement: true
        }
    }

    pub fn new_gc<A: 'static>(&mut self, value: A) -> Gc<A> {
        let ctx: *mut GcCtx = self;
        Gc {
            ctx: ctx,
            node: Box::into_raw(Box::new(Node {
                count: 1,
                colour: Colour::Black,
                buffered: false,
                children: Vec::new(),
                weak_nodes: Vec::new(),
                data: Box::new(value) as Box<Any>
            })),
            phantom: PhantomData
        }
    }

    fn increment(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        s.count = s.count + 1;
        s.colour = Colour::Black;
    }

    fn decrement(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        s.count = s.count - 1;
        if s.count == 0 {
            self.release(s);
        } else {
            self.possible_root(s);
        }
    }

    fn release(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        for child in &s.children {
            self.decrement(*child);
        }
        s.colour = Colour::Black;
        if !s.buffered {
            self.system_free(s);
        }
    }

    fn system_free(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        unsafe {
            Box::from_raw(s);
        }
    }

    fn possible_root(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        if s.colour != Colour::Purple {
            s.colour = Colour::Purple;
            if !s.buffered {
                s.buffered = true;
                self.roots.push(s);
            }
        }
    }

    pub fn collect_cycles(&mut self) {
        self.mark_roots();
        self.scan_roots();
        self.collect_roots();
    }

    fn mark_roots(&mut self) {
        let roots = self.roots.clone();
        for s in roots {
            let s = unsafe { &mut *s };
            if s.colour == Colour::Purple && s.count > 0 {
                self.mark_gray(s);
            } else {
                s.buffered = false;
                self.roots.retain(|s2| !ptr::eq(s, *s2));
                if s.colour == Colour::Black && s.count == 0 {
                    self.system_free(s);
                }
            }
        }
    }

    fn scan_roots(&mut self) {
        let roots = self.roots.clone();
        for s in roots {
            self.scan(s);
        }
    }

    fn collect_roots(&mut self) {
        let roots = self.roots.clone();
        self.roots.clear();
        for s in roots {
            let s = unsafe { &mut *s };
            s.buffered = false;
            self.collect_white(s);
        }
    }

    fn mark_gray(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        if s.colour != Colour::Gray {
            s.colour = Colour::Gray;
            for t in &s.children {
                let t = unsafe { &mut **t };
                t.count = t.count - 1;
                self.mark_gray(t);
            }
        }
    }

    fn scan(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        if s.colour == Colour::Gray {
            if s.count > 0 {
                self.scan_black(s);
            } else {
                s.colour = Colour::White;
                for t in &s.children {
                    let t = unsafe { &mut **t };
                    self.scan(t);
                }
            }
        }
    }

    fn scan_black(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        s.colour = Colour::Black;
        for t in &s.children {
            let t = unsafe { &mut **t };
            t.count = t.count + 1;
            if t.colour != Colour::Black {
                self.scan_black(t);
            }
        }
    }

    fn collect_white(&mut self, s: *mut Node) {
        let s = unsafe { &mut *s };
        if s.colour == Colour::White && !s.buffered {
            s.colour = Colour::Black;
            for t in &s.children {
                self.collect_white(*t);
            }
            self.system_free(s);
        }
    }
}
