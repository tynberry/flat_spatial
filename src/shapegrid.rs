use crate::cell::{CellObject, ShapeGridCell};
use crate::shape::{Shape, AABB};
use crate::storage::{SparseStorage, Storage};
use mint::Point2;
use slotmap::new_key_type;
use slotmap::SlotMap;
use std::iter::Filter;

pub type ShapeGridObjects<O, Idx> = SlotMap<ShapeGridHandle, StoreObject<O, Idx>>;

new_key_type! {
    /// This handle is used to modify the associated object or to update its position.
    /// It is returned by the _insert_ method of a ShapeGrid.
    pub struct ShapeGridHandle;
}

/// The actual object stored in the store
#[derive(Clone, Copy)]
pub struct StoreObject<O: Copy, S: Shape> {
    /// User-defined object to be associated with a value
    obj: O,
    pub shape: S,
}

/// ShapeGrid is a generic shape-based spatial partitioning structure that uses a generic storage of cells which acts as a
/// grid instead of a tree.
///
/// ## Fast queries
/// In theory, ShapeGrid should be faster than a quadtree/r-tree because it has no log costs
/// (calculating the cells around a point is trivial).  
/// However, it only works if the cell size is adapted to the problem, much like how a tree has to
/// be balanced to be efficient.  
///
/// ## Dynamicity
/// ShapeGrid's allows eager removals and position updates, however for big shapes (spanning many cells)
/// this can be expensive, so beware.
///
/// Use this grid for mostly static objects with the occasional removal/position update if needed.
///
/// A SlotMap is used for objects managing, adding a level of indirection between shapes and objects.
/// SlotMap is used because removal doesn't alter handles given to the user, while still having constant time access.
/// However it requires O to be copy, but SlotMap's author stated that they were working on a similar
/// map where Copy isn't required.
///
/// ## About object managment
///
/// In theory, you don't have to use the object managment directly, you can make your custom
/// Handle -> Object map by specifying "`()`" to be the object type.
/// _(This can be useful if your object is not Copy)_
/// Since `()` is zero sized, it should probably optimize away a lot of the object managment code.
///
/// ```rust
/// use flat_spatial::ShapeGrid;
/// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
/// let handle = g.insert([0.0, 0.0], ());
/// // Use handle however you want
/// ```
///
/// ## Examples
/// Here is a basic example that shows most of its capabilities:
/// ```rust
/// use flat_spatial::ShapeGrid;
///
/// let mut g: ShapeGrid<i32> = ShapeGrid::new(10); // Creates a new grid with a cell width of 10 with an integer as extra data
/// let a = g.insert([0.0, 0.0], 0); // Inserts a new element with data: 0
///
/// {
///     let mut before = g.query_around([0.0, 0.0], 5.0).map(|(id, _pos)| id); // Queries for objects around a given point
///     assert_eq!(before.next(), Some(a));
///     assert_eq!(g.get(a).unwrap().1, &0);
/// }
/// let b = g.insert([0.0, 0.0], 1); // Inserts a new element, assigning a new unique and stable handle, with data: 1
///
/// g.remove(a); // Removes a value using the handle given by `insert`
///
/// assert_eq!(g.handles().collect::<Vec<_>>(), vec![b]); // We check that the "a" object has been removed
///
/// let after: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|(id, _pos)| id).collect(); // And that b is query-able
/// assert_eq!(after, vec![b]);
///
/// assert_eq!(g.get(b).unwrap().1, &1); // We also check that b still has his data associated
/// assert_eq!(g.get(a), None); // But that a doesn't exist anymore
/// ```
#[derive(Clone)]
pub struct ShapeGrid<O: Copy, S: Shape = AABB, ST: Storage = SparseStorage> {
    storage: ST,
    objects: ShapeGridObjects<O, S>,
}

impl<S: Shape, ST: Storage, O: Copy> ShapeGrid<O, S, ST> {
    /// Creates an empty grid.   
    /// The cell size should be about the same magnitude as your queries size.
    pub fn new(cell_size: i32) -> Self {
        Self {
            storage: ST::new(cell_size),
            objects: SlotMap::with_key(),
        }
    }

    /// Creates an empty grid.   
    /// The cell size should be about the same magnitude as your queries size.
    pub fn with_storage(st: ST) -> Self {
        Self {
            storage: st,
            objects: SlotMap::with_key(),
        }
    }

    fn cell_ids(storage: &mut ST, shape: &S) -> impl Iterator<Item = ST::IdxIter> {
        let bbox = shape.bbox();
        let ll = storage.cell_id(bbox.ll);
        let ur = storage.cell_id(bbox.ur);
        storage
            .cell_range(ll, ur)
            .filter(|id| shape.intersects(storage.cell_aabb(id)))
    }

    fn cell_mut<'a>(
        storage: &'a mut ST,
        objects: &mut ShapeGridObjects<O, ST::Idx>,
        pos: Point2<f32>,
    ) -> (ST::Idx, &'a mut ShapeGridCell) {
        storage.cell_mut(pos, move |storage| {
            storage.modify(move |cell| cell.objs.clear());

            for (handle, obj) in objects.iter_mut() {
                for id in cell_ids(storage, &obj.shape) {
                    storage.cell_mut_unchecked(id).objs.push((handle, obj.pos));
                }
            }
        })
    }

    /// Inserts a new object with a position and an associated object
    /// Returns the unique and stable handle to be used with get_obj
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], ());
    /// ```
    pub fn insert(&mut self, pos: impl Into<Point2<f32>>, obj: O) -> ShapeGridHandle {
        let pos = pos.into();

        let Self {
            storage, objects, ..
        } = self;

        let (cell_id, cell) = Self::cell_mut(storage, objects, pos);
        let handle = objects.insert(StoreObject {
            obj,
            state: ObjectState::Unchanged,
            pos,
            cell_id,
        });
        cell.objs.push((handle, pos));
        handle
    }

    /// Lazily sets the position of an object (if it is not marked for deletion).
    /// This won't be taken into account until maintain() is called.  
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], ());
    /// g.set_position(h, [3.0, 3.0]);
    /// ```
    pub fn set_position(&mut self, handle: ShapeGridHandle, pos: impl Into<Point2<f32>>) {
        let pos = pos.into();

        let obj = self
            .objects
            .get_mut(handle)
            .expect("Object not in grid anymore");
        obj.pos = pos;
        if obj.state != ObjectState::Removed {
            let target_id = self.storage.cell_id(pos);

            obj.state = if target_id == obj.cell_id {
                ObjectState::NewPos
            } else {
                ObjectState::Relocate
            };
        }

        self.storage.cell_mut_unchecked(obj.cell_id).dirty = true;
    }

    /// Lazily removes an object from the grid.
    /// This won't be taken into account until maintain() is called.  
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], ());
    /// g.remove(h);
    /// ```
    pub fn remove(&mut self, handle: ShapeGridHandle) {
        let st = self
            .objects
            .get_mut(handle)
            .expect("Object not in grid anymore");

        st.state = ObjectState::Removed;
        self.storage.cell_mut_unchecked(st.cell_id).dirty = true;
    }

    /// Maintains the world, updating all the positions (and moving them to corresponding cells)
    /// and removing necessary objects and empty cells.
    /// Runs in linear time O(N) where N is the number of objects.
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], ());
    /// g.remove(h);
    ///
    /// assert!(g.get(h).is_some());
    /// g.maintain();
    /// assert!(g.get(h).is_none());
    /// ```
    pub fn maintain(&mut self) {
        let Self {
            storage,
            objects,
            to_relocate,
            ..
        } = self;

        storage.modify(|cell| cell.maintain(objects, to_relocate));

        for (handle, pos) in to_relocate.drain(..) {
            Self::cell_mut(storage, objects, pos)
                .1
                .objs
                .push((handle, pos));
        }
    }

    /// Iterate over all handles
    pub fn handles(&self) -> impl Iterator<Item = ShapeGridHandle> + '_ {
        self.objects.keys()
    }

    /// Returns a reference to the associated object and its position, using the handle.  
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<i32> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], 42);
    /// assert_eq!(g.get(h), Some(([5.0, 3.0].into(), &42)));
    /// ```
    pub fn get(&self, id: ShapeGridHandle) -> Option<(Point2<f32>, &O)> {
        self.objects.get(id).map(|x| (x.pos, &x.obj))
    }

    /// Returns a mutable reference to the associated object and its position, using the handle.  
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    /// let mut g: ShapeGrid<i32> = ShapeGrid::new(10);
    /// let h = g.insert([5.0, 3.0], 42);
    /// *g.get_mut(h).unwrap().1 = 56;
    /// assert_eq!(g.get(h).unwrap().1, &56);
    /// ```    
    pub fn get_mut(&mut self, id: ShapeGridHandle) -> Option<(Point2<f32>, &mut O)> {
        self.objects.get_mut(id).map(|x| (x.pos, &mut x.obj))
    }

    /// The underlying storage
    pub fn storage(&self) -> &ST {
        &self.storage
    }

    /// Queries for all objects around a position within a certain radius.
    /// Try to keep the radius asked and the cell size of similar magnitude for better performance.
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    ///
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let a = g.insert([0.0, 0.0], ());
    ///
    /// let around: Vec<_> = g.query_around([2.0, 2.0], 5.0).map(|(id, _pos)| id).collect();
    ///
    /// assert_eq!(vec![a], around);
    /// ```
    pub fn query_around(
        &self,
        pos: impl Into<Point2<f32>>,
        radius: f32,
    ) -> impl Iterator<Item = CellObject> + '_ {
        let pos = pos.into();

        let ll = [pos.x - radius, pos.y - radius].into(); // lower left
        let ur = [pos.x + radius, pos.y + radius].into(); // upper right

        let radius2 = radius * radius;
        self.query_raw(ll, ur).filter(move |(_, pos_obj)| {
            let x = pos_obj.x - pos.x;
            let y = pos_obj.y - pos.y;
            x * x + y * y < radius2
        })
    }

    /// Queries for all objects in an aabb (aka a rect).
    /// Try to keep the rect's width/height of similar magnitudes to the cell size for better performance.
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    ///
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let a = g.insert([0.0, 0.0], ());
    ///
    /// let around: Vec<_> = g.query_aabb([-1.0, -1.0], [1.0, 1.0]).map(|(id, _pos)| id).collect();
    ///
    /// assert_eq!(vec![a], around);
    /// ```
    pub fn query_aabb(
        &self,
        aa: impl Into<Point2<f32>>,
        bb: impl Into<Point2<f32>>,
    ) -> impl Iterator<Item = CellObject> + '_ {
        let aa = aa.into();
        let bb = bb.into();

        let ll = [aa.x.min(bb.x), aa.y.min(bb.y)].into(); // lower left
        let ur = [aa.x.max(bb.x), aa.y.max(bb.y)].into(); // upper right

        self.query_raw(ll, ur).filter(move |(_, pos_obj)| {
            (ll.x..=ur.x).contains(&pos_obj.x) && (ll.y..=ur.y).contains(&pos_obj.y)
        })
    }

    /// Queries for all objects in the cells intersecting an axis-aligned rectangle defined by lower left (ll) and upper right (ur)
    /// Try to keep the rect's width/height of similar magnitudes to the cell size for better performance.
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    ///
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let a = g.insert([0.0, 0.0], ());
    /// let b = g.insert([5.0, 5.0], ());
    ///
    /// let around: Vec<_> = g.query_raw([-1.0, -1.0].into(), [1.0, 1.0].into()).map(|(id, _pos)| id).collect();
    ///
    /// assert_eq!(vec![a, b], around);
    /// ```
    pub fn query_raw(
        &self,
        ll: Point2<f32>,
        ur: Point2<f32>,
    ) -> impl Iterator<Item = CellObject> + '_ {
        let ll_id = self.storage.cell_id(ll);
        let ur_id = self.storage.cell_id(ur);

        self.storage
            .cell_range(ll_id, ur_id)
            .flat_map(move |id| self.storage.cell(id))
            .flat_map(|x| x.objs.iter().copied())
    }

    /// Allows to look directly at what's in a cell covering a specific position.
    ///
    /// # Example
    /// ```rust
    /// use flat_spatial::ShapeGrid;
    ///
    /// let mut g: ShapeGrid<()> = ShapeGrid::new(10);
    /// let a = g.insert([2.0, 2.0], ());
    ///
    /// let around = g.get_cell([1.0, 1.0]).collect::<Vec<_>>();
    ///
    /// assert_eq!(vec![(a, [2.0, 2.0].into())], around);
    /// ```
    pub fn get_cell(
        &mut self,
        pos: impl Into<mint::Point2<f32>>,
    ) -> impl Iterator<Item = CellObject> + '_ {
        self.storage
            .cell(self.storage.cell_id(pos.into()))
            .into_iter()
            .flat_map(|x| x.objs.iter().copied())
    }

    /// Returns the number of objects currently available
    /// (removals that were not confirmed with maintain() are still counted)
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Checks if the grid contains objects or not
    /// (removals that were not confirmed with maintain() are still counted)
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ShapeGrid;

    #[test]
    fn test_small_query() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);
        let a = g.insert([5.0, 0.0], ());
        let b = g.insert([11.0, 0.0], ());
        let c = g.insert([5.0, 8.0], ());

        let near: Vec<_> = g.query_around([6.0, 0.0], 2.0).map(|x| x.0).collect();
        assert_eq!(near, vec![a]);

        let mid: Vec<_> = g.query_around([8.0, 0.0], 4.0).map(|x| x.0).collect();
        assert!(mid.contains(&a));
        assert!(mid.contains(&b));

        let far: Vec<_> = g.query_around([6.0, 0.0], 10.0).map(|x| x.0).collect();
        assert!(far.contains(&a));
        assert!(far.contains(&b));
        assert!(far.contains(&c));
    }

    #[test]
    fn test_big_query_around() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);

        for i in 0..100 {
            g.insert([i as f32, 0.0], ());
        }

        let q: Vec<_> = g.query_around([15.0, 0.0], 9.5).map(|x| x.0).collect();
        assert_eq!(q.len(), 19); // 1 middle, 8 left, 8 right
    }

    #[test]
    fn test_big_query_rect() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);

        for i in 0..100 {
            g.insert([i as f32, 0.0], ());
        }

        let q: Vec<_> = g
            .query_aabb([5.5, 1.0], [15.5, -1.0])
            .map(|x| x.0)
            .collect();
        assert_eq!(q.len(), 10);
    }

    #[test]
    fn test_distance_test() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);
        let a = g.insert([3.0, 4.0], ());

        let far: Vec<_> = g.query_around([0.0, 0.0], 5.1).map(|x| x.0).collect();
        assert_eq!(far, vec![a]);

        let near: Vec<_> = g.query_around([0.0, 0.0], 4.9).map(|x| x.0).collect();
        assert_eq!(near, vec![]);
    }

    #[test]
    fn test_change_position() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);
        let a = g.insert([0.0, 0.0], ());

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![a]);

        g.set_position(a, [30.0, 30.0]);
        g.maintain();

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![]);

        let after: Vec<_> = g.query_around([30.0, 30.0], 5.0).map(|x| x.0).collect();
        assert_eq!(after, vec![a]);
    }

    #[test]
    fn test_remove() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);
        let a = g.insert([0.0, 0.0], ());

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![a]);

        g.remove(a);
        let b = g.insert([0.0, 0.0], ());
        g.maintain();

        assert_eq!(g.handles().collect::<Vec<_>>(), vec![b]);

        let after: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(after, vec![b]);
    }

    #[test]
    fn test_resize() {
        let mut g: ShapeGrid<()> = ShapeGrid::new(10);
        let a = g.insert([-1000.0, 0.0], ());

        let q: Vec<_> = g.query_around([-1000.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(q, vec![a]);

        let b = g.insert([0.0, 1000.0], ());

        let q: Vec<_> = g.query_around([0.0, 1000.0], 5.0).map(|x| x.0).collect();
        assert_eq!(q, vec![b]);
    }
}

#[cfg(test)]
mod testssparse {
    use crate::SparseShapeGrid;

    #[test]
    fn test_small_query() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);
        let a = g.insert([5.0, 0.0], ());
        let b = g.insert([11.0, 0.0], ());
        let c = g.insert([5.0, 8.0], ());

        let near: Vec<_> = g.query_around([6.0, 0.0], 2.0).map(|x| x.0).collect();
        assert_eq!(near, vec![a]);

        let mid: Vec<_> = g.query_around([8.0, 0.0], 4.0).map(|x| x.0).collect();
        assert!(mid.contains(&a));
        assert!(mid.contains(&b));

        let far: Vec<_> = g.query_around([6.0, 0.0], 10.0).map(|x| x.0).collect();
        assert!(far.contains(&a));
        assert!(far.contains(&b));
        assert!(far.contains(&c));
    }

    #[test]
    fn test_big_query_around() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);

        for i in 0..100 {
            g.insert([i as f32, 0.0], ());
        }

        let q: Vec<_> = g.query_around([15.0, 0.0], 9.5).map(|x| x.0).collect();
        assert_eq!(q.len(), 19); // 1 middle, 8 left, 8 right
    }

    #[test]
    fn test_big_query_rect() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);

        for i in 0..100 {
            g.insert([i as f32, 0.0], ());
        }

        let q: Vec<_> = g
            .query_aabb([5.5, 1.0], [15.5, -1.0])
            .map(|x| x.0)
            .collect();
        assert_eq!(q.len(), 10);
    }

    #[test]
    fn test_distance_test() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);
        let a = g.insert([3.0, 4.0], ());

        let far: Vec<_> = g.query_around([0.0, 0.0], 5.1).map(|x| x.0).collect();
        assert_eq!(far, vec![a]);

        let near: Vec<_> = g.query_around([0.0, 0.0], 4.9).map(|x| x.0).collect();
        assert_eq!(near, vec![]);
    }

    #[test]
    fn test_change_position() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);
        let a = g.insert([0.0, 0.0], ());

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![a]);

        g.set_position(a, [30.0, 30.0]);
        g.maintain();

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![]);

        let after: Vec<_> = g.query_around([30.0, 30.0], 5.0).map(|x| x.0).collect();
        assert_eq!(after, vec![a]);
    }

    #[test]
    fn test_remove() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);
        let a = g.insert([0.0, 0.0], ());

        let before: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(before, vec![a]);

        g.remove(a);
        let b = g.insert([0.0, 0.0], ());
        g.maintain();

        assert_eq!(g.handles().collect::<Vec<_>>(), vec![b]);

        let after: Vec<_> = g.query_around([0.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(after, vec![b]);
    }

    #[test]
    fn test_resize() {
        let mut g: SparseShapeGrid<()> = SparseShapeGrid::new(10);
        let a = g.insert([-1000.0, 0.0], ());

        let q: Vec<_> = g.query_around([-1000.0, 0.0], 5.0).map(|x| x.0).collect();
        assert_eq!(q, vec![a]);

        let b = g.insert([0.0, 1000.0], ());

        let q: Vec<_> = g.query_around([0.0, 1000.0], 5.0).map(|x| x.0).collect();
        assert_eq!(q, vec![b]);
    }
}
