//! AEMS create-message schemas. Bounds and fixed fields follow the TU3
//! wrapper instructions.
//! This models the message format, not the guest implementation or allocator.

#[derive(Clone, Copy, Debug)]
pub enum Message {
    FootDrag, Skid, Flips, Seams, Squeaks, Rattle, Wind, SpeedRattle,
    BoardSlide, BodySlide, ClothFalls, Footsteps,
}

impl Message {
    pub fn pack(self, args: &[i32]) -> Vec<i32> {
        // (word count, initial nonzero words, argument -> word/low/high).
        type Field = (usize, i32, i32);
        let (n, fixed, fields): (usize, &[(usize, i32)], &[Field]) = match self {
            Self::FootDrag => (15, &[(1,32767),(5,25000)],
                &[(7,0,10000),(8,0,10),(9,0,32767),(10,0,32767),(11,0,32767),(12,0,32767),(13,0,2),(14,0,32767)]),
            Self::Skid => (18, &[(1,32767),(5,25000)],
                &[(7,0,10000),(8,0,1),(9,0,4),(10,0,90),(11,0,32767),(12,0,32767),
                  (13,0,1),(14,0,1),(15,0,1),(16,0,32767),(17,0,32767)]),
            Self::Flips => (28, &[(1,32767),(4,4096),(5,25000)],
                &[(7,0,1000),(8,0,1000),(9,0,1000),(10,0,1000),(11,0,40),(16,0,1),
                  (17,0,32767),(18,0,32767),(19,0,32767),(21,0,32767),(23,0,1),
                  (24,0,1),(25,0,1),(26,0,32767),(27,0,32767)]),
            Self::Seams => (20, &[(1,32767),(4,4096),(5,25000)],
                &[(10,0,8),(11,0,1),(14,0,3),(19,0,32767)]),
            Self::Squeaks => (11, &[(1,32767),(4,4096),(5,25000),(8,15)],
                &[(7,0,1000),(9,0,1000),(10,0,32767)]),
            Self::Rattle => (12, &[(2,4096),(6,1),(8,25000),(10,32767)],
                &[(3,0,10000),(4,0,8),(11,0,32767)]),
            Self::Wind => (13, &[(2,4096),(4,25000)],
                &[(3,0,1000),(8,0,32767),(9,0,32767),(10,0,32767),(11,0,32767),(12,0,32767)]),
            Self::SpeedRattle => (11, &[(2,4096),(4,25000)],
                &[(3,0,1000),(8,0,32767),(9,0,32767),(10,0,32767)]),
            Self::BoardSlide => (12, &[(2,4096),(4,25000)], &[(8,0,32767),(10,0,3)]),
            Self::BodySlide => (12, &[(4,25000)], &[(3,0,1000),(8,0,4),(9,0,1),(11,0,32767)]),
            Self::ClothFalls => (10, &[(4,25000)], &[(3,0,1000),(9,0,32767)]),
            Self::Footsteps => (25, &[(13,1)], &[(0,0,32767),(1,0,65535),(2,0,8192),(3,0,1000),
                (4,0,25001),(5,0,25001),(6,0,32767),(7,0,32767),(8,0,1),(9,0,1000),
                (10,0,1000),(11,0,1),(12,1,4),(14,0,1000),(15,0,5),(16,1,7),(17,1,5),
                (18,0,32767),(19,0,32767),(20,0,32767),(21,0,32767),(22,0,32767),
                (23,0,32767),(24,0,32767)]),
        };
        assert_eq!(args.len(), fields.len(), "wrong create-message argument count for {self:?}");
        let mut words = vec![0; n];
        for &(i, value) in fixed { words[i] = value; }
        for (&arg, &(i, low, high)) in args.iter().zip(fields) { words[i] = arg.clamp(low, high); }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wind_creation_clamps_inputs_and_initializes_the_dry_message() {
        assert_eq!(Message::Wind.pack(&[-7, 7, 32768, 17000, 17000, 6750]),
            [0, 0, 4096, 0, 25000, 0, 0, 0, 7, 32767, 17000, 17000, 6750]);
    }
    #[test]
    fn drag_message_preserves_its_contact_role_and_tuning_fields() {
        assert_eq!(Message::FootDrag.pack(&[500,0,4000,4500,4000,22500,1,7]),
            [0,32767,0,0,0,25000,0,500,0,4000,4500,4000,22500,1,7]);
    }
}
